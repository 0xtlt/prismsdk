#!/usr/bin/env python3
"""Security review S33: a fake browser for a desktop sign-in, given the
authorization URL of scripts/mock-oidc.py. Before the user approves, it sends
the app's loopback listener callbacks that are not the sign-in's: another
path, another state, two states, no state. Then it approves, as the user
would, and follows the provider's redirect to the real callback. Each step
goes to $S33_LOG (the state and code redacted). The sign-in must still end,
signed in, with the real callback.

    s33-browser.py <authorization URL>

On Linux it is the default browser through a desktop entry (see
scripts/linux-sign-in.sh); Windows has scripts/windows/s33-browser.ps1.
"""
import http.client
import os
import re
import sys
import time
import urllib.parse

auth = sys.argv[1]
log = os.environ.get("S33_LOG", "/tmp/s33-browser.log")
query = dict(urllib.parse.parse_qsl(urllib.parse.urlsplit(auth).query))
state = query["state"]


def say(line):
    line = re.sub(r"code=[^&\s]+", "code=<code>", line.replace(state, "<state>"))
    with open(log, "a", encoding="utf-8") as f:
        f.write(line + "\n")


def get(url):
    """The status and Location of a GET (redirects not followed)."""
    u = urllib.parse.urlsplit(url)
    c = http.client.HTTPConnection(u.hostname, u.port, timeout=10)
    try:
        c.request("GET", u.path + ("?" + u.query if u.query else ""))
        r = c.getresponse()
        return r.status, r.getheader("Location")
    except OSError as e:
        return f"error ({e.__class__.__name__})", None
    finally:
        c.close()


redirect = urllib.parse.urlsplit(query["redirect_uri"])
say(f"authorize: redirect_uri={query['redirect_uri']}")
base = f"http://127.0.0.1:{redirect.port}"
for path in [
    f"/callbackX?code=evil&state={state}",
    f"/callback/x?code=evil&state={state}",
    "/callback?code=evil&state=another-state",
    f"/callback?code=evil&state={state}&state={state}",
    "/callback?code=evil",
    f"/?code=evil&state={state}",
    "/favicon.ico",
]:
    status, _ = get(base + path)
    say(f"not the sign-in's: {path} -> {status}")
time.sleep(1)
# The user approves: the provider redirects to the real callback.
status, location = get(auth.replace("/authorize?", "/approve?") + "&decision=allow")
say(f"approved ({status}): the provider redirects to {location}")
status, _ = get(location)
say(f"the real callback -> {status}")
