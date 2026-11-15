#!/usr/bin/env python3
"""A local OpenID Connect provider for testing sign-in (`prism::oauth`), with no
real accounts: one test user, codes bound to their PKCE challenge (S256).

    python3 scripts/mock-oidc.py [--port 8765] [--host 127.0.0.1]

Endpoints: /.well-known/openid-configuration, /authorize (a consent page),
/token (authorization_code with PKCE, refresh_token). The ID token is unsigned
(alg "none"): fine for a local test, never for production. Every step is logged,
so a test can check that the app sent the verifier matching its challenge.
"""
import argparse
import base64
import hashlib
import html
import json
import secrets
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

CODES = {}
REFRESH = {}
USER = {"sub": "test-user-1", "email": "test.user@example.com", "name": "Test User"}


def b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def id_token(issuer, client_id, nonce):
    now = int(time.time())
    claims = dict(USER, iss=issuer, aud=client_id, iat=now, exp=now + 3600)
    if nonce:
        claims["nonce"] = nonce
    head = b64url(json.dumps({"alg": "none", "typ": "JWT"}).encode())
    return f"{head}.{b64url(json.dumps(claims).encode())}."


class Handler(BaseHTTPRequestHandler):
    def issuer(self):
        # As the client sees it (10.0.2.2 from the Android emulator…).
        return f"http://{self.headers.get('Host', 'localhost')}"

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
        url = urllib.parse.urlsplit(self.path)
        q = dict(urllib.parse.parse_qsl(url.query))
        if url.path == "/.well-known/openid-configuration":
            i = self.issuer()
            return self.send(200, json.dumps({
                "issuer": i,
                "authorization_endpoint": f"{i}/authorize",
                "token_endpoint": f"{i}/token",
                "response_types_supported": ["code"],
                "code_challenge_methods_supported": ["S256"],
                "id_token_signing_alg_values_supported": ["none"],
            }))
        if url.path == "/authorize":
            missing = [k for k in ("client_id", "redirect_uri", "state", "code_challenge") if k not in q]
            if missing or q.get("response_type") != "code" or q.get("code_challenge_method") != "S256":
                return self.send(400, f"bad request: missing {missing} or not code+S256", "text/plain")
            print(f"[mock-oidc] authorize client={q['client_id']} redirect={q['redirect_uri']} "
                  f"challenge={q['code_challenge']} scope={q.get('scope')}", flush=True)
            hidden = "".join(f'<input type="hidden" name="{html.escape(k)}" value="{html.escape(v)}">' for k, v in q.items())
            page = f"""<!doctype html><meta charset=utf-8><meta name=viewport content="width=device-width,initial-scale=1">
<title>Prism ID</title>
<style>body{{font:17px -apple-system,system-ui,sans-serif;margin:0;display:grid;place-items:center;min-height:100vh;background:#f4f4f8}}
main{{background:#fff;padding:28px;border-radius:16px;box-shadow:0 2px 12px #0001;max-width:340px;text-align:center}}
button{{font:inherit;width:100%;padding:12px;border-radius:10px;border:0;margin-top:10px}}
.ok{{background:#4f46e5;color:#fff}}.no{{background:#eee}}</style>
<main><h1>Prism ID</h1><p>A local test provider.<br><b>{html.escape(q['client_id'])}</b> wants your name and email.</p>
<form action="/approve">{hidden}<button class=ok id=approve name=decision value=allow>Continue as Test User</button>
<button class=no id=deny name=decision value=deny>Cancel</button></form></main>"""
            return self.send(200, page, "text/html; charset=utf-8")
        if url.path == "/approve":
            if q.get("decision") != "allow":
                return self.redirect(q["redirect_uri"], error="access_denied", state=q["state"])
            code = secrets.token_urlsafe(24)
            CODES[code] = q
            print(f"[mock-oidc] approved: code issued for {q['redirect_uri']}", flush=True)
            return self.redirect(q["redirect_uri"], code=code, state=q["state"])
        return self.send(404, "not found", "text/plain")

    def do_POST(self):
        if urllib.parse.urlsplit(self.path).path != "/token":
            return self.send(404, "not found", "text/plain")
        body = self.rfile.read(int(self.headers.get("Content-Length", 0))).decode()
        f = dict(urllib.parse.parse_qsl(body))
        err = lambda e, d: self.send(400, json.dumps({"error": e, "error_description": d}))
        if f.get("grant_type") == "refresh_token":
            client = REFRESH.get(f.get("refresh_token"))
            if not client:
                return err("invalid_grant", "unknown refresh token")
            return self.send(200, json.dumps({"access_token": secrets.token_urlsafe(32), "token_type": "Bearer", "expires_in": 3600}))
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
        return self.send(200, json.dumps({
            "access_token": secrets.token_urlsafe(32),
            "token_type": "Bearer",
            "expires_in": 3600,
            "refresh_token": refresh,
            "id_token": id_token(self.issuer(), a["client_id"], a.get("nonce")),
            "scope": a.get("scope", ""),
        }))

    def log_message(self, fmt, *args):
        print("[mock-oidc]", fmt % args, flush=True)


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8765)
    ap.add_argument("--host", default="127.0.0.1")
    a = ap.parse_args()
    print(f"[mock-oidc] issuer http://{a.host}:{a.port}", flush=True)
    ThreadingHTTPServer((a.host, a.port), Handler).serve_forever()
