#!/usr/bin/env python3
"""A local OpenID Connect / OAuth 2 provider for testing sign-in (`prism::oauth`),
with no real accounts: one test user, codes bound to their PKCE challenge (S256).

    python3 scripts/mock-oidc.py [--port 8765] [--host 127.0.0.1]
                                 [--path /realm] [--no-oidc] [--secret] [--token T]

Endpoints (under the issuer, `http://<host>:<port><path>`):

- `/.well-known/openid-configuration` (OpenID Connect discovery; not with
  `--no-oidc`, a plain OAuth 2 server) and RFC 8414 metadata at
  `/.well-known/oauth-authorization-server<path>` (the path inserted after the
  host, as RFC 8414 says);
- `/register`: RFC 7591 dynamic client registration (JSON). Clients registered
  here must use a registered redirect URI (a loopback one on any port) and, if
  they got a secret (`--secret` hands one out even to public clients), send it;
- `/authorize` (a consent page), `/token` (authorization_code with PKCE,
  refresh_token);
- `/picture`: a per-user PNG that needs `Authorization: Bearer <access token>`
  (for `picture(url).header(…)`); `--token T` makes `T` a valid one too.

The ID token is unsigned (alg "none"): fine for a local test, never for
production. Every step is logged, so a test can check that the app sent the
verifier matching its challenge.
"""
import argparse
import base64
import hashlib
import html
import json
import secrets
import struct
import time
import urllib.parse
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

CODES = {}
REFRESH = {}
ACCESS = set()
CLIENTS = {}
USER = {"sub": "test-user-1", "email": "test.user@example.com", "name": "Test User"}
OPTS = argparse.Namespace(path="", no_oidc=False, secret=False)


def b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def id_token(issuer, client_id, nonce):
    now = int(time.time())
    claims = dict(USER, iss=issuer, aud=client_id, iat=now, exp=now + 3600)
    if nonce:
        claims["nonce"] = nonce
    head = b64url(json.dumps({"alg": "none", "typ": "JWT"}).encode())
    return f"{head}.{b64url(json.dumps(claims).encode())}."


def without_loopback_port(uri):
    u = urllib.parse.urlsplit(uri)
    if u.scheme == "http" and u.hostname in ("127.0.0.1", "::1", "localhost"):
        host = f"[{u.hostname}]" if ":" in u.hostname else u.hostname
        return urllib.parse.urlunsplit((u.scheme, host, u.path, u.query, u.fragment))
    return uri


def png(size=96):
    """The test user's avatar: a violet disc with a lighter ring."""
    rows = []
    c = (size - 1) / 2
    for y in range(size):
        row = bytearray([0])
        for x in range(size):
            d = ((x - c) ** 2 + (y - c) ** 2) ** 0.5 / c
            if d > 1:
                row += bytes([0, 0, 0, 0])
            elif d > 0.8:
                row += bytes([196, 181, 253, 255])
            else:
                t = y / size
                row += bytes([int(79 + 60 * t), int(70 + 20 * t), 229, 255])
        rows.append(bytes(row))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(b"".join(rows))) + chunk(b"IEND", b""))


class Handler(BaseHTTPRequestHandler):
    def origin(self):
        # As the client sees it (10.0.2.2 from the Android emulator…).
        return f"http://{self.headers.get('Host', 'localhost')}"

    def issuer(self):
        return self.origin() + OPTS.path

    def metadata(self):
        i = self.issuer()
        return {
            "issuer": i,
            "authorization_endpoint": f"{i}/authorize",
            "token_endpoint": f"{i}/token",
            "registration_endpoint": f"{i}/register",
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "token_endpoint_auth_methods_supported": ["none", "client_secret_post", "client_secret_basic"],
            "code_challenge_methods_supported": ["S256"],
            "id_token_signing_alg_values_supported": ["none"],
        }

    def route(self):
        """The path inside the issuer (None: not ours)."""
        url = urllib.parse.urlsplit(self.path)
        if url.path == "/.well-known/oauth-authorization-server" + OPTS.path:
            return "8414", url
        if not url.path.startswith(OPTS.path + "/"):
            return None, url
        return url.path[len(OPTS.path):], url

    def send(self, status, body, ctype="application/json", headers=()):
        data = body.encode() if isinstance(body, str) else body
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        for k, v in headers:
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(data)

    def redirect(self, uri, **params):
        sep = "&" if "?" in uri else "?"
        self.send(302, "", "text/plain", [("Location", uri + sep + urllib.parse.urlencode(params))])

    def do_GET(self):
        path, url = self.route()
        q = dict(urllib.parse.parse_qsl(url.query))
        if path == "8414":
            print("[mock-oidc] RFC 8414 metadata", flush=True)
            return self.send(200, json.dumps(self.metadata()))
        if path == "/.well-known/openid-configuration" and not OPTS.no_oidc:
            return self.send(200, json.dumps(self.metadata()))
        if path == "/picture":
            auth = self.headers.get("Authorization", "")
            if not auth.startswith("Bearer ") or auth[7:] not in ACCESS:
                print("[mock-oidc] picture: no valid bearer token", flush=True)
                return self.send(401, "{}", headers=[("WWW-Authenticate", "Bearer")])
            print("[mock-oidc] picture: bearer token ok", flush=True)
            return self.send(200, png(), "image/png")
        if path == "/authorize":
            missing = [k for k in ("client_id", "redirect_uri", "state", "code_challenge") if k not in q]
            if missing or q.get("response_type") != "code" or q.get("code_challenge_method") != "S256":
                return self.send(400, f"bad request: missing {missing} or not code+S256", "text/plain")
            client = CLIENTS.get(q["client_id"])
            if client and without_loopback_port(q["redirect_uri"]) not in client["redirects"]:
                return self.send(400, "redirect_uri not registered for this client", "text/plain")
            print(f"[mock-oidc] authorize client={q['client_id']} redirect={q['redirect_uri']} "
                  f"challenge={q['code_challenge']} scope={q.get('scope')}", flush=True)
            name = client["name"] if client else q["client_id"]
            hidden = "".join(f'<input type="hidden" name="{html.escape(k)}" value="{html.escape(v)}">' for k, v in q.items())
            page = f"""<!doctype html><meta charset=utf-8><meta name=viewport content="width=device-width,initial-scale=1">
<title>Prism ID</title>
<style>body{{font:17px -apple-system,system-ui,sans-serif;margin:0;display:grid;place-items:center;min-height:100vh;background:#f4f4f8}}
main{{background:#fff;padding:28px;border-radius:16px;box-shadow:0 2px 12px #0001;max-width:340px;text-align:center}}
button{{font:inherit;width:100%;padding:12px;border-radius:10px;border:0;margin-top:10px}}
.ok{{background:#4f46e5;color:#fff}}.no{{background:#eee}}</style>
<main><h1>Prism ID</h1><p>A local test provider.<br><b>{html.escape(name)}</b> wants your name and email.</p>
<form action="{OPTS.path}/approve">{hidden}<button class=ok id=approve name=decision value=allow>Continue as Test User</button>
<button class=no id=deny name=decision value=deny>Cancel</button></form></main>"""
            return self.send(200, page, "text/html; charset=utf-8")
        if path == "/approve":
            if q.get("decision") != "allow":
                return self.redirect(q["redirect_uri"], error="access_denied", state=q["state"])
            code = secrets.token_urlsafe(24)
            CODES[code] = q
            print(f"[mock-oidc] approved: code issued for {q['redirect_uri']}", flush=True)
            return self.redirect(q["redirect_uri"], code=code, state=q["state"])
        return self.send(404, "not found", "text/plain")

    def do_POST(self):
        path, _ = self.route()
        body = self.rfile.read(int(self.headers.get("Content-Length", 0))).decode()
        if path == "/register":
            return self.register(body)
        if path != "/token":
            return self.send(404, "not found", "text/plain")
        f = dict(urllib.parse.parse_qsl(body))
        err = lambda e, d, status=400: self.send(status, json.dumps({"error": e, "error_description": d}))
        # Client authentication: HTTP Basic or in the form (RFC 6749 §2.3.1).
        basic = self.headers.get("Authorization", "")
        if basic.startswith("Basic "):
            cid, _, secret = base64.b64decode(basic[6:]).decode().partition(":")
            f["client_id"], f["client_secret"] = urllib.parse.unquote_plus(cid), urllib.parse.unquote_plus(secret)
        client = CLIENTS.get(f.get("client_id", ""))
        if client and client["secret"] and f.get("client_secret") != client["secret"]:
            return err("invalid_client", "wrong or missing client secret", 401)
        if f.get("grant_type") == "refresh_token":
            owner = REFRESH.get(f.get("refresh_token"))
            if not owner:
                return err("invalid_grant", "unknown refresh token")
            if owner != f.get("client_id"):
                return err("invalid_grant", "refresh token issued to another client")
            print(f"[mock-oidc] token: refreshed for {owner}", flush=True)
            access = secrets.token_urlsafe(32)
            ACCESS.add(access)
            return self.send(200, json.dumps({"access_token": access, "token_type": "Bearer", "expires_in": 3600}))
        if f.get("grant_type") != "authorization_code":
            return err("unsupported_grant_type", f.get("grant_type"))
        a = CODES.pop(f.get("code", ""), None)
        if not a:
            return err("invalid_grant", "unknown or used code")
        if f.get("redirect_uri") != a["redirect_uri"] or f.get("client_id") != a["client_id"]:
            return err("invalid_grant", "redirect_uri or client_id mismatch")
        verifier = f.get("code_verifier", "")
        if b64url(hashlib.sha256(verifier.encode()).digest()) != a["code_challenge"]:
            print("[mock-oidc] token: PKCE verifier does NOT match the challenge", flush=True)
            return err("invalid_grant", "PKCE verification failed")
        print(f"[mock-oidc] token: PKCE ok (verifier {len(verifier)} chars), tokens issued", flush=True)
        refresh = secrets.token_urlsafe(32)
        REFRESH[refresh] = a["client_id"]
        access = secrets.token_urlsafe(32)
        ACCESS.add(access)
        return self.send(200, json.dumps({
            "access_token": access,
            "token_type": "Bearer",
            "expires_in": 3600,
            "refresh_token": refresh,
            "id_token": id_token(self.issuer(), a["client_id"], a.get("nonce")),
            "scope": a.get("scope", ""),
        }))

    def register(self, body):
        """RFC 7591 §3: a client from its metadata."""
        err = lambda e, d: self.send(400, json.dumps({"error": e, "error_description": d}))
        try:
            meta = json.loads(body)
        except ValueError:
            return err("invalid_client_metadata", "not JSON")
        redirects = meta.get("redirect_uris")
        if not isinstance(redirects, list) or not redirects or not all(isinstance(u, str) for u in redirects):
            return err("invalid_redirect_uri", "redirect_uris: a non-empty list")
        method = meta.get("token_endpoint_auth_method", "client_secret_basic")
        if method not in ("none", "client_secret_post", "client_secret_basic"):
            return err("invalid_client_metadata", f"token_endpoint_auth_method {method}")
        if OPTS.secret and method == "none":
            method = "client_secret_basic"  # a server making confidential clients anyway
        cid = "dyn-" + secrets.token_urlsafe(9)
        secret = secrets.token_urlsafe(24) if method != "none" else None
        CLIENTS[cid] = {"name": meta.get("client_name", cid), "secret": secret,
                        "redirects": {without_loopback_port(u) for u in redirects}}
        print(f"[mock-oidc] registered {cid} ({meta.get('client_name')}, {method}) for {redirects}", flush=True)
        answer = dict(meta, client_id=cid, client_id_issued_at=int(time.time()),
                      token_endpoint_auth_method=method)
        if secret:
            answer.update(client_secret=secret, client_secret_expires_at=0)
        return self.send(201, json.dumps(answer))

    def log_message(self, fmt, *args):
        print("[mock-oidc]", fmt % args, flush=True)


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8765)
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--path", default="", help="issuer path, e.g. /realm")
    ap.add_argument("--no-oidc", action="store_true", help="RFC 8414 metadata only (a plain OAuth 2 server)")
    ap.add_argument("--secret", action="store_true", help="registration hands out client secrets")
    ap.add_argument("--token", help="an access token accepted without signing in (screenshots)")
    a = ap.parse_args()
    if a.token:
        ACCESS.add(a.token)
    OPTS.path, OPTS.no_oidc, OPTS.secret = a.path.rstrip("/"), a.no_oidc, a.secret
    print(f"[mock-oidc] issuer http://{a.host}:{a.port}{OPTS.path}", flush=True)
    ThreadingHTTPServer((a.host, a.port), Handler).serve_forever()
