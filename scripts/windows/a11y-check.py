#!/usr/bin/env python3
"""Checks UI Automation trees (uia-dump.ps1 -Json) against expectations.

  a11y-check.py EXPECT.json DIR [CASE...]

For each case of EXPECT.json (or only those named), reads DIR/<case>.json
and checks that:

- every pattern of "has" matches at least one element ("count": exactly
  that many);
- no pattern of "lacks" matches an element;
- the elements of "order" come in that order in the tree (the reading
  order, and the order the Tab key follows in XAML).

A pattern is an object of an element's keys (name, type, localizedType,
toggle, selected, expanded, value, range, disabled, focusable, invoke,
heading, live, dialog, landmark, help, position, className) and the values
they must have. A text starting with "~" is a regular expression searched
in the element's. A key the dump leaves out has no value (null): a state the
element does not have, or false for disabled, focusable, invoke and dialog.
"within" is a pattern for one of the element's ancestors.

Each failure is printed with the elements whose name matches; the exit code
is 1 when any check fails. Standard library only.
"""

import json
import os
import re
import sys

FLAGS = ("disabled", "focusable", "invoke", "dialog")


def flatten(nodes, ancestors=()):
    for n in nodes:
        yield n, ancestors
        yield from flatten(n.get("children") or [], ancestors + (n,))


def value(node, key):
    if key in node:
        return node[key]
    return False if key in FLAGS else None


def matches(node, ancestors, pattern):
    for key, want in pattern.items():
        if key == "within":
            if not any(matches(a, (), want) for a in ancestors):
                return False
            continue
        if key == "count":
            continue
        got = value(node, key)
        if isinstance(want, str) and want.startswith("~"):
            if not isinstance(got, str) or not re.search(want[1:], got):
                return False
        elif got != want:
            return False
    return True


def describe(node):
    keys = [k for k in node if k not in ("children", "bounds", "automationId")]
    return "{" + ", ".join(f"{k}={node[k]!r}" for k in keys) + "}"


def near(elements, pattern):
    """Elements with the pattern's name (or type), to show what is there."""
    name = pattern.get("name")
    if isinstance(name, str) and not name.startswith("~"):
        found = [n for n, _ in elements if n.get("name") == name]
    else:
        found = [n for n, _ in elements if n.get("type") == pattern.get("type")]
    return found[:4]


def check_case(case, tree):
    elements = list(flatten(tree if isinstance(tree, list) else [tree]))
    failures = []
    for pattern in case.get("has", []):
        hits = [n for n, a in elements if matches(n, a, pattern)]
        want = pattern.get("count")
        ok = len(hits) == want if want is not None else bool(hits)
        if not ok:
            lines = [f"expected {want if want is not None else 'an'} element(s) like {pattern}, found {len(hits)}"]
            lines += [f"    near: {describe(n)}" for n in near(elements, pattern)]
            failures.append("\n".join(lines))
    # Reading order: XAML moves the keyboard focus in the same tree order.
    order = case.get("order", [])
    at = -1
    for pattern in order:
        hits = [i for i, (n, a) in enumerate(elements) if i > at and matches(n, a, pattern)]
        if not hits:
            before = [i for i, (n, a) in enumerate(elements) if matches(n, a, pattern)]
            where = "before the previous one" if before else "nowhere"
            failures.append(f"reading order: {pattern} comes {where}")
            break
        at = hits[0]
    for pattern in case.get("lacks", []):
        hits = [n for n, a in elements if matches(n, a, pattern)]
        if hits:
            failures.append(f"expected no element like {pattern}, found {describe(hits[0])}")
    return failures


def main(argv):
    if len(argv) < 3:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    expect = json.load(open(argv[1], encoding="utf-8"))
    folder, only = argv[2], set(argv[3:])
    failed = 0
    checked = 0
    for case in expect["cases"]:
        if only and case["name"] not in only:
            continue
        path = os.path.join(folder, case["name"] + ".json")
        if not os.path.exists(path):
            print(f"a11y-check: FAIL: {case['name']}: no {path}", file=sys.stderr)
            failed += 1
            continue
        tree = json.load(open(path, encoding="utf-8-sig"))
        failures = check_case(case, tree)
        checked += 1
        n = len(case.get("has", [])) + len(case.get("lacks", [])) + bool(case.get("order"))
        if failures:
            failed += 1
            for f in failures:
                print(f"a11y-check: FAIL: {case['name']}: {f}", file=sys.stderr)
        else:
            print(f"a11y-check: {case['name']} ({case.get('path', '/')}): {n} checks OK")
    if failed:
        print(f"a11y-check: {failed} case(s) failed", file=sys.stderr)
        return 1
    print(f"a11y-check: {checked} cases OK")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
