#!/usr/bin/env python3
"""Checks the AT-SPI trees scripts/linux-a11y.sh dumped.

  a11y-check.py EXPECT.json DUMP-FOLDER [case…]

For each case of EXPECT.json (all of them, or the ones named), reads
<case>.json in DUMP-FOLDER and runs its checks. Every case runs the
"always" checks too:

  no-type-names     no element is named after its GTK or Prism type
                    ("PrismBox", "GtkToggleButton"), as GTK 4.8 names an
                    element without a label
  controls-focusable  every enabled button, link, tab, check box, radio
                    and switch takes the keyboard focus (itself, or the
                    control GTK puts inside it)
  named-controls    every button, link, tab, check box, radio, switch,
                    table row and cell has a name (a link of a label's
                    text is read from the label's text)
  one-selected-tab  the tab bar has one selected tab

Checks with parameters (EXPECT.json, per case):

  {"tabs": [names…]}            the tab bar's tabs, in order
  {"table": {"name", "columns", "rows"}}
                                a table: its header row (header cells named
                                after the columns), at least `rows` data
                                rows, each named after its content and
                                made of cells described by their column's
                                header (DESCRIBED_BY: Orca reads the
                                header's text after the cell), each cell's
                                content still an element of its own
  {"element": {"role", "name", "focusable"?}}  such an element exists

Exits non-zero, listing every failure, when a check fails.
"""

import json
import re
import sys

TYPE_NAME = re.compile(r"^(Gtk|Prism|Adw)[A-Z]\w*$")
CONTROLS = {"button", "toggle button", "link", "page tab", "check box", "radio button", "switch"}
# Role names that changed with at-spi2-core versions.
ROLE_ALIASES = {"push button": "button"}
NAMED = CONTROLS | {"table row", "table cell"}
# GTK 4.8 to 4.24 show a column header as a filler (gtkatspiutils.c),
# 4.25 and later as a column header.
HEADERS = {"filler", "column header", "table column header"}


def walk(nodes, parent=None):
    for n in nodes:
        n["role"] = ROLE_ALIASES.get(n["role"], n["role"])
        n["_parent"] = parent
        yield n
        yield from walk(n.get("children", []), n)


def label(n):
    name = n.get("name")
    return f'{n["role"]} "{name}"' if name else n["role"]


def path(n):
    parts = []
    while n is not None:
        parts.append(label(n))
        n = n["_parent"]
    return " > ".join(reversed(parts[:-1]))


def check_no_type_names(nodes, _):
    return [f"named after its type: {path(n)}" for n in nodes if TYPE_NAME.match(n.get("name", ""))]


def focusable(n):
    """`n` or, for GTK's composite controls (a GtkMenuButton and its inner
    toggle), a control inside it takes the focus."""
    return "focusable" in n["states"] or any(focusable(c) for c in n.get("children", []))


def check_controls_focusable(nodes, _):
    out = []
    for n in nodes:
        if n["role"] in CONTROLS and "sensitive" in n["states"] and not focusable(n):
            out.append(f"not focusable: {path(n)}")
    return out


def gtk_hyperlink(n):
    """A link of a label's text (GTK 4.16 and later): read from the label's
    text, it has no name of its own."""
    parent = n["_parent"]
    return "Hyperlink" in n["interfaces"] and parent is not None and "Hypertext" in parent["interfaces"]


def check_named_controls(nodes, _):
    return [
        f"no name: {path(n)}"
        for n in nodes
        if n["role"] in NAMED and not n.get("name") and not gtk_hyperlink(n)
    ]


def tab_lists(nodes):
    return [n for n in nodes if n["role"] == "page tab list"]


def check_one_selected_tab(nodes, _):
    out = []
    for bar in tab_lists(nodes):
        tabs = [c for c in bar.get("children", []) if c["role"] == "page tab"]
        selected = [t for t in tabs if "selected" in t["states"]]
        if len(selected) != 1:
            out.append(f"{len(selected)} selected tabs in {path(bar)}")
    return out


def check_tabs(nodes, names):
    bars = tab_lists(nodes)
    if not bars:
        return ["no tab bar (page tab list)"]
    tabs = [c.get("name", "") for c in bars[0].get("children", []) if c["role"] == "page tab"]
    return [] if tabs == names else [f"tabs {tabs}, expected {names}"]


def check_table(nodes, spec):
    tables = [n for n in nodes if n["role"] == "table" and n.get("name") == spec["name"]]
    if not tables:
        return [f'no table "{spec["name"]}"']
    table = tables[0]
    out = []
    rows = [c for c in table.get("children", []) if c["role"] == "table row"]
    if not rows:
        return [f"no table row in {path(table)}"]
    header = rows[0].get("children", [])
    names = [h.get("name", "") for h in header]
    if names != spec["columns"]:
        out.append(f"header cells {names}, expected {spec['columns']}")
    for h in header:
        if h["role"] not in HEADERS:
            out.append(f"header cell is a {h['role']}: {path(h)}")
    data = rows[1:]
    if len(data) < spec["rows"]:
        out.append(f"{len(data)} data rows, expected at least {spec['rows']}")
    for row in data:
        cells = row.get("children", [])
        if any(c["role"] != "table cell" for c in cells):
            out.append(f"not only table cells in {path(row)}")
        content = " ".join(c.get("name", "") for c in cells)
        if row.get("name") != content:
            out.append(f'row named "{row.get("name", "")}", its cells read "{content}": {path(row)}')
        for h, cell in zip(header, cells):
            by = [r["targets"] for r in cell.get("relations", []) if r["type"] == "described-by"]
            if by != [[h["id"]]]:
                out.append(f'cell not described by its header "{h.get("name", "")}": {path(cell)}')
            if not cell.get("children"):
                out.append(f"cell content not an element of its own: {path(cell)}")
    return out


def check_element(nodes, spec):
    for n in nodes:
        if n["role"] == spec["role"] and n.get("name") == spec["name"]:
            focusable = "focusable" in n["states"]
            if "focusable" in spec and focusable != spec["focusable"]:
                return [f"{label(n)}: focusable is {focusable}, expected {spec['focusable']}"]
            return []
    return [f'no {spec["role"]} "{spec["name"]}"']


SIMPLE = {
    "no-type-names": check_no_type_names,
    "controls-focusable": check_controls_focusable,
    "named-controls": check_named_controls,
    "one-selected-tab": check_one_selected_tab,
}
WITH_PARAMS = {"tabs": check_tabs, "table": check_table, "element": check_element}


def run(check, nodes):
    if isinstance(check, str):
        return check, SIMPLE[check](nodes, None)
    (kind, spec), = check.items()
    return kind, WITH_PARAMS[kind](nodes, spec)


def main():
    expect_path, folder, *only = sys.argv[1:]
    expect = json.load(open(expect_path, encoding="utf-8"))
    failures = 0
    for case in expect["cases"]:
        if only and case["name"] not in only:
            continue
        tree = json.load(open(f"{folder}/{case['name']}.json", encoding="utf-8"))
        nodes = list(walk(tree))
        for check in expect["always"] + case.get("checks", []):
            kind, problems = run(check, nodes)
            for p in problems:
                print(f"linux-a11y: FAIL: {case['name']}: {kind}: {p}")
            failures += len(problems)
    if failures:
        sys.exit(f"linux-a11y: {failures} failures")
    print("linux-a11y: all checks pass")


if __name__ == "__main__":
    main()
