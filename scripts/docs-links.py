#!/usr/bin/env python3
"""Checks the links of the built book (standard library only).

Every relative link must reach a file, and every #anchor must exist in the
page it points to. Absolute links must start with the book's `site-url`
(docs/book.toml). The API docs (`api/`) are copied in at deploy time, so
links into them are not checked. External links are not fetched.

    mdbook build docs
    scripts/docs-links.py docs/book
"""

import html.parser
import os
import re
import sys
import urllib.parse

ROOT = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "docs/book")
BOOK_TOML = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "docs", "book.toml")
with open(BOOK_TOML, encoding="utf-8") as f:
    m = re.search(r'^site-url\s*=\s*"([^"]+)"', f.read(), re.M)
SITE = m.group(1) if m else "/"
# Pages mdBook generates whole, whose links repeat the chapters' own.
SKIP = {"print.html", "404.html", "toc.html"}


class Page(html.parser.HTMLParser):
    def __init__(self):
        super().__init__()
        self.links = []
        self.ids = set()

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if a.get("id"):
            self.ids.add(a["id"])
        if tag == "a" and a.get("name"):
            self.ids.add(a["name"])
        for key in ("href", "src"):
            if a.get(key):
                self.links.append(a[key])


def main():
    pages = {}
    for dirpath, _, files in os.walk(ROOT):
        for name in files:
            if name.endswith(".html"):
                path = os.path.join(dirpath, name)
                page = Page()
                with open(path, encoding="utf-8") as f:
                    page.feed(f.read())
                pages[path] = page
    if not pages:
        sys.exit(f"docs-links: no pages in {ROOT} (run `mdbook build docs` first)")

    bad = []
    for path, page in sorted(pages.items()):
        if os.path.basename(path) in SKIP:
            continue
        here = os.path.relpath(path, ROOT)
        for link in page.links:
            u = urllib.parse.urlsplit(link)
            if u.scheme or link.startswith("//"):
                continue
            if not u.path:
                if not u.fragment:
                    continue
                target = path
            else:
                if u.path.startswith("/"):
                    if not u.path.startswith(SITE):
                        bad.append(f"{here}: {link}: outside the site ({SITE})")
                        continue
                    target = os.path.join(ROOT, urllib.parse.unquote(u.path[len(SITE):]))
                else:
                    target = os.path.join(os.path.dirname(path), urllib.parse.unquote(u.path))
                target = os.path.normpath(target)
                if os.path.isdir(target):
                    target = os.path.join(target, "index.html")
                if os.path.relpath(target, ROOT).split(os.sep)[0] == "api":
                    continue
                if not os.path.exists(target):
                    bad.append(f"{here}: {link}: no such file")
                    continue
            if u.fragment and target in pages:
                if urllib.parse.unquote(u.fragment) not in pages[target].ids:
                    bad.append(f"{here}: {link}: no such anchor")

    for line in bad:
        print(f"docs-links: {line}")
    print(f"docs-links: {len(pages)} pages, {len(bad)} broken links")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
