"""lldb module for scripts/macos-apple-sign-in.sh (security review S39).

Sign in with Apple needs the com.apple.developer.applesignin entitlement and
an Apple ID, so the app's real request fails here. This module lets the app
build and send its real request, records what Apple would receive, and
replaces only Apple's answer with a MOCK credential:

- at `prism_apple_sign_in` (Rust -> Swift): the request id and the nonce and
  state Rust passes to the Swift bridge;
- at the bridge's `authorizationController(controller:didCompleteWithError:)`
  (AuthenticationServices answering): the nonce and state of the request the
  ASAuthorizationController holds, and the error it reports; then, instead of
  that error, a credential whose identity token is a mock JWT (HS256 with a
  test secret, not Apple's RS256) carrying the request's nonce as its `nonce`
  claim, as Apple does, and the state $S39_REPLY_STATE.

Environment: S39_OUT (JSON written there), S39_SECRET (HMAC key of the mock
token), S39_AUD (the bundle id), S39_REPLY_STATE (the state of the answer).
"""

import base64
import hashlib
import hmac
import json
import os
import time

import lldb

seen = {}

DELEGATE_ERROR = (
    "$s11PrismBridge0A11AppleSignInC23authorizationController10controller"
    "20didCompleteWithErrorySo015ASAuthorizationG0C_s0L0_ptFTo"
)


def _cstring(process, address):
    if not address:
        return None
    error = lldb.SBError()
    s = process.ReadCStringFromMemory(address, 4096, error)
    return s if error.Success() else None


def _objc_string(frame, expression):
    v = frame.EvaluateExpression("(const char *)[(NSString *)(%s) UTF8String]" % expression)
    return _cstring(frame.GetThread().GetProcess(), v.GetValueAsUnsigned())


def _b64(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def mock_token(nonce):
    """A JWT shaped like Apple's identity token, signed with the test secret."""
    now = int(time.time())
    header = {"alg": "HS256", "kid": "s39-mock", "typ": "JWT"}
    claims = {
        "iss": "https://appleid.apple.com",
        "aud": os.environ["S39_AUD"],
        "sub": "001.s39mock",
        "iat": now,
        "exp": now + 600,
        "nonce": nonce,
        "nonce_supported": True,
        "email": "s39@example.com",
    }
    signing = _b64(json.dumps(header).encode()) + "." + _b64(json.dumps(claims).encode())
    sig = hmac.new(os.environ["S39_SECRET"].encode(), signing.encode(), hashlib.sha256).digest()
    return signing + "." + _b64(sig)


def on_sign_in(frame, bp_loc, extra_args, internal_dict):
    process = frame.GetThread().GetProcess()
    reg = lambda r: frame.FindRegister(r).GetValueAsUnsigned()
    # prism_apple_sign_in(id, email, name, nonce, state)
    seen["id"] = reg("x0")
    seen["bridge_nonce"] = _cstring(process, reg("x3"))
    seen["bridge_state"] = _cstring(process, reg("x4"))
    return False


def on_apple_answer(frame, bp_loc, extra_args, internal_dict):
    # -[PrismAppleSignIn authorizationController:didCompleteWithError:]:
    # x0 self, x1 _cmd, x2 the controller, x3 the error.
    request = "[[(id)$x2 authorizationRequests] firstObject]"
    seen["request_class"] = _objc_string(frame, "NSStringFromClass((Class)[(id)%s class])" % request)
    seen["request_nonce"] = _objc_string(frame, "[(id)%s nonce]" % request)
    seen["request_state"] = _objc_string(frame, "[(id)%s state]" % request)
    seen["apple_error"] = "%s %d" % (
        _objc_string(frame, "[(NSError *)$x3 domain]"),
        frame.EvaluateExpression("(long)[(NSError *)$x3 code]").GetValueAsSigned(),
    )
    credential = {
        "user": "001.s39mock",
        "identity_token": mock_token(seen["request_nonce"]),
        "authorization_code": "s39-mock-code",
        "email": "s39@example.com",
        "state": os.environ.get("S39_REPLY_STATE", ""),
    }
    seen["credential"] = credential
    literal = json.dumps(credential).replace("\\", "\\\\").replace('"', '\\"')
    target = frame.GetThread().GetProcess().GetTarget()
    symbol = target.FindSymbols("prism_device_reply_fields")[0].GetSymbol()
    address = symbol.GetStartAddress().GetLoadAddress(target)
    reply = frame.EvaluateExpression(
        '((void (*)(unsigned long long, const char *))0x%x)(%dULL, "%s")'
        % (address, seen["id"], literal)
    )
    error = reply.GetError()
    # A void expression "fails" with no result: that is the call done.
    seen["replied"] = error.Success() or error.GetError() == 0x1001 or "no value" in str(error)
    seen["reply_error"] = str(error)
    # Skip the bridge's own failure answer.
    seen["skipped"] = str(frame.GetThread().ReturnFromFrame(frame, lldb.SBValue()))
    with open(os.environ["S39_OUT"], "w") as f:
        json.dump(seen, f, indent=1)
    return False


def __lldb_init_module(debugger, internal_dict):
    target = debugger.GetSelectedTarget()
    for symbol, callback in (
        ("prism_apple_sign_in", "on_sign_in"),
        (DELEGATE_ERROR, "on_apple_answer"),
    ):
        bp = target.BreakpointCreateByName(symbol)
        bp.SetScriptCallbackFunction(__name__ + "." + callback)
