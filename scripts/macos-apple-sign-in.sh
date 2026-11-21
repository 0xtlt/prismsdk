#!/bin/bash
# Security review S39 on macOS: Sign in with Apple sends Apple the SHA-256
# (lower-case hex) of the server's nonce, so the identity token's `nonce`
# claim is that hash and a server checks it as the docs say (hash the nonce
# it handed out, compare); a credential whose state is not the request's is
# refused.
#
# Real Sign in with Apple needs the entitlement and an Apple ID: the app
# sends its real request to AuthenticationServices (which fails without the
# entitlement), and lldb (scripts/s39_apple_lldb.py) replaces only Apple's
# answer with a MOCK credential. Its identity token is a mock JWT (HS256 with
# a test secret, not signed by Apple) carrying the request's nonce, as
# Apple's does.
#
#   scripts/macos-apple-sign-in.sh <out dir> [path/to/Counter.app]
#
# Needs the counter's bundle (`prism run macos --bundle` in examples/counter).
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
out=$(cd "$1" && pwd)
app=${2:-$repo/target/macos/Counter.app}
app="$(cd "$(dirname "$app")" && pwd)/$(basename "$app")"
plist="$app/Contents/Info.plist"
bin="$app/Contents/MacOS/$(/usr/libexec/PlistBuddy -c "Print :CFBundleExecutable" "$plist")"
S39_AUD=$(/usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" "$plist")
S39_SECRET=$(openssl rand -hex 32)
export S39_AUD S39_SECRET
nonce="s39-server-nonce-$(openssl rand -hex 8)"
state="s39-state-$(openssl rand -hex 4)"
echo "server nonce: $nonce, state: $state"

run() { # name, the answer's state
  rm -f "$out/macos-s39-$1.json" "$out/macos-s39-$1.png"
  S39_OUT="$out/macos-s39-$1.json" S39_REPLY_STATE="$2" \
    PRISM_PATH=/settings PRISM_LOCALE=en PRISM_APPLE_NONCE="$nonce" PRISM_APPLE_STATE="$state" \
    PRISM_CLICK="Sign in with Apple" PRISM_AFTER=3 PRISM_SNAPSHOT="$out/macos-s39-$1.png" \
    xcrun lldb -b -o "command script import $repo/scripts/s39_apple_lldb.py" -o run -- "$bin" \
    >"$out/macos-s39-$1.lldb.log" 2>&1 || true
  [ -s "$out/macos-s39-$1.json" ] || { tail -20 "$out/macos-s39-$1.lldb.log"; echo "FAIL $1: the request did not reach AuthenticationServices"; exit 1; }
}
run same-state "$state"
run other-state "s39-state-other"

# The server's side: verify the token as the docs say.
python3 - "$out" "$nonce" "$state" <<'EOF'
import base64, hashlib, hmac, json, os, sys, time
out, nonce, state = sys.argv[1:]
expected = hashlib.sha256(nonce.encode()).hexdigest()
failed = False
def check(ok, what):
    global failed
    failed |= not ok
    print(("ok   " if ok else "FAIL ") + what)

def b64(s):
    return base64.urlsafe_b64decode(s + "=" * (-len(s) % 4))

def verify(token, server_nonce):
    """What a server does with the identity token (mock: HS256, not Apple's RS256 keys)."""
    head, body, sig = token.split(".")
    mac = hmac.new(os.environ["S39_SECRET"].encode(), (head + "." + body).encode(), hashlib.sha256).digest()
    if not hmac.compare_digest(mac, b64(sig)):
        return "bad signature"
    c = json.loads(b64(body))
    if c["iss"] != "https://appleid.apple.com" or c["aud"] != os.environ["S39_AUD"] or c["exp"] < time.time():
        return "bad iss, aud or exp"
    if c["nonce"] != hashlib.sha256(server_nonce.encode()).hexdigest():
        return "nonce claim %s… is not SHA-256(server nonce)" % c["nonce"][:12]
    return "accepted"

for name in ("same-state", "other-state"):
    s = json.load(open(os.path.join(out, "macos-s39-%s.json" % name)))
    print("-- %s: request %s; AuthenticationServices answered %s (replaced by the mock credential)"
          % (name, s["request_class"], s["apple_error"]))
    check(s["bridge_nonce"] == expected, "Rust passed the bridge SHA-256(nonce) = %s…" % s["bridge_nonce"][:16])
    check(s["request_nonce"] == expected, "the ASAuthorizationAppleIDRequest's nonce is SHA-256(nonce), not the nonce")
    check(s["request_state"] == state, "the request's state is the app's state")
    check(nonce not in json.dumps(s), "the server's nonce itself never reached Apple's request")
    check(s["replied"] and s["skipped"] == "success",
          "the mock credential (state %s) went to the app instead of that error" % s["credential"]["state"])
    token = s["credential"]["identity_token"]
    r = verify(token, nonce)
    check(r == "accepted", "the server's check of the (mock) identity token: %s" % r)

# Before the fix the bridge passed the nonce unhashed, so Apple's token
# carried the nonce itself: the documented check refuses it.
s = json.load(open(os.path.join(out, "macos-s39-same-state.json")))
head, body, _ = s["credential"]["identity_token"].split(".")
c = json.loads(b64(body))
def mint(claims):
    enc = lambda d: base64.urlsafe_b64encode(json.dumps(d).encode()).rstrip(b"=").decode()
    signing = head + "." + enc(claims)
    sig = hmac.new(os.environ["S39_SECRET"].encode(), signing.encode(), hashlib.sha256).digest()
    return signing + "." + base64.urlsafe_b64encode(sig).rstrip(b"=").decode()
print("-- the same check on other tokens")
r = verify(mint(dict(c, nonce=nonce)), nonce)
check(r != "accepted", "a token carrying the nonce unhashed (the bridge before the fix): %s" % r)
r = verify(mint(c), "s39-another-server-nonce")
check(r != "accepted", "this token against another sign-in's nonce: %s" % r)
sys.exit(1 if failed else 0)
EOF
echo "macos-apple-sign-in: look at macos-s39-same-state.png (signed in as s39@example.com) and macos-s39-other-state.png (refused: another request)"
