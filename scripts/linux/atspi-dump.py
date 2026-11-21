#!/usr/bin/env python3
"""Dumps an app's AT-SPI tree: what Orca and other screen readers get.

  atspi-dump.py APP-NAME OUT.json [--text OUT.txt] [--wait SECONDS]
                [--press ROLE NAME KEY]... [--back]

Waits (up to --wait seconds, 30 by default) for an application called
APP-NAME on the accessibility bus, then for its window to have content,
and writes its tree as JSON: for each element its role, name,
description, states, attributes, interfaces, relations (the targets'
"id"s), and when the element implements them its Table and TableCell
data (counts, its row and column, its column headers), its text and its
value. "--text" also writes an indented, one line per element version
for reading. The application's element also records the toolkit and
its version (GTK 4.8.3…).

"--press" first moves the keyboard focus to the element with that role
and name (with Tab, or Shift+Tab with "--back", as a keyboard user: GTK
doesn't implement AT-SPI's GrabFocus), then presses KEY (an X key name:
Return, space), both with xdotool (XTest); then the tree is dumped.

Needs the AT-SPI bus (at-spi-bus-launcher, at-spi2-registryd), the
Atspi typelib (gir1.2-atspi-2.0) and, for "--press", xdotool: see
scripts/linux-a11y.sh.
"""

import argparse
import json
import subprocess
import sys
import time

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi  # noqa: E402

# Every `Atspi.RelationType` and `Atspi.StateType` by value.
RELATIONS = {
    int(v): v.value_nick for v in Atspi.RelationType.__enum_values__.values()
}
STATES = {int(v): v.value_nick for v in Atspi.StateType.__enum_values__.values()}


def find_app(name, wait):
    deadline = time.monotonic() + wait
    while True:
        desktop = Atspi.get_desktop(0)
        for i in range(desktop.get_child_count()):
            app = desktop.get_child_at_index(i)
            if app is not None and app.get_name() == name:
                # Its window and the window's content.
                if app.get_child_count() > 0:
                    window = app.get_child_at_index(0)
                    if window is not None and window.get_child_count() > 0:
                        return app
        if time.monotonic() > deadline:
            sys.exit(f"atspi-dump: no application '{name}' with a window on the bus")
        time.sleep(0.5)


# Role names that changed with at-spi2-core versions.
ROLE_ALIASES = {"push button": "button"}


def find(acc, role, name):
    found_role = safe(acc.get_role_name)
    if ROLE_ALIASES.get(found_role, found_role) == role and safe(acc.get_name) == name:
        return acc
    for i in range(safe(acc.get_child_count, 0)):
        child = safe(lambda: acc.get_child_at_index(i))
        if child is not None and (found := find(child, role, name)) is not None:
            return found
    return None


def key_press(key_name):
    """Presses an X key (XTest, through xdotool): AT-SPI's own key
    synthesis doesn't reach GTK 4 windows under Xvfb."""
    subprocess.run(["xdotool", "key", key_name], check=True)


def press(app, role, name, key_name, back):
    target = find(app, role, name)
    if target is None:
        sys.exit(f'atspi-dump: no {role} "{name}" to press {key_name} on')
    # GTK doesn't implement AT-SPI's GrabFocus: Tab to it, as a keyboard
    # user does.
    for _ in range(300):
        if Atspi.StateType.FOCUSED in target.get_state_set().get_states():
            break
        key_press("shift+Tab" if back else "Tab")
        time.sleep(0.1)
    else:
        sys.exit(f'atspi-dump: Tab never reaches {role} "{name}"')
    key_press(key_name)
    time.sleep(1)
    print(f'atspi-dump: pressed {key_name} on {role} "{name}"')


def key(acc):
    """A stable identity of an accessible: its object path (one app)."""
    return acc.path


def safe(f, default=None):
    try:
        return f()
    except Exception:  # A widget gone, an interface that answers nothing.
        return default


def walk(acc, ids, out, depth=0):
    node = {"id": len(ids)}
    ids[key(acc)] = node["id"]
    node["role"] = safe(acc.get_role_name, "?")
    name = safe(acc.get_name, "")
    if name:
        node["name"] = name
    description = safe(acc.get_description, "")
    if description:
        node["description"] = description
    states = safe(lambda: acc.get_state_set().get_states(), [])
    node["states"] = sorted(STATES.get(int(s), str(s)) for s in states)
    attributes = safe(acc.get_attributes, {}) or {}
    attributes.pop("toolkit", None)
    if attributes:
        node["attributes"] = dict(sorted(attributes.items()))
    interfaces = safe(acc.get_interfaces, []) or []
    node["interfaces"] = sorted(i for i in interfaces if i != "Accessible")
    relations = []
    for rel in safe(acc.get_relation_set, []) or []:
        targets = [rel.get_target(i) for i in range(rel.get_n_targets())]
        relations.append(
            {
                "type": RELATIONS.get(int(rel.get_relation_type()), "?"),
                "targets": [key(t) for t in targets if t is not None],
            }
        )
    if relations:
        node["relations"] = relations
    if "Table" in interfaces:
        node["table"] = {
            "rows": safe(lambda: Atspi.Table.get_n_rows(acc)),
            "columns": safe(lambda: Atspi.Table.get_n_columns(acc)),
        }
    if "TableCell" in interfaces:
        pos = safe(lambda: Atspi.TableCell.get_position(acc))
        node["cell"] = {
            "position": list(pos)[1:] if pos else None,
            "columnHeaders": [
                key(h) for h in safe(lambda: Atspi.TableCell.get_column_header_cells(acc), [])
            ],
        }
    if "Text" in interfaces:
        text = safe(lambda: Atspi.Text.get_text(acc, 0, -1), "")
        if text and text != name:
            node["text"] = text
    if "Value" in interfaces:
        node["value"] = safe(lambda: Atspi.Value.get_current_value(acc))
    out.append(node)
    children = []
    for i in range(safe(acc.get_child_count, 0)):
        child = safe(lambda: acc.get_child_at_index(i))
        if child is not None:
            walk(child, ids, children, depth + 1)
    if children:
        node["children"] = children


def resolve(nodes, ids):
    """Relation and header targets as the "id"s of the dump's elements."""
    for n in nodes:
        for rel in n.get("relations", []):
            rel["targets"] = [ids.get(t, "outside") for t in rel["targets"]]
        if "cell" in n:
            n["cell"]["columnHeaders"] = [
                ids.get(t, "outside") for t in n["cell"]["columnHeaders"]
            ]
        resolve(n.get("children", []), ids)


def text_lines(nodes, by_id, depth=0):
    for n in nodes:
        parts = [n["role"]]
        if "toolkit" in n:
            parts.append(f"{n['toolkit']['name']} {n['toolkit']['version']}")
        if "name" in n:
            parts.append(json.dumps(n["name"], ensure_ascii=False))
        if "description" in n:
            parts.append("description=" + json.dumps(n["description"], ensure_ascii=False))
        if "attributes" in n:
            parts.append(
                "attrs{" + ", ".join(f"{k}={v}" for k, v in n["attributes"].items()) + "}"
            )
        if "table" in n:
            parts.append(f"Table(rows={n['table']['rows']}, columns={n['table']['columns']})")
        if "cell" in n:
            parts.append(f"TableCell(position={n['cell']['position']})")
        for rel in n.get("relations", []):
            names = [by_id[t].get("name", by_id[t]["role"]) if t in by_id else t for t in rel["targets"]]
            parts.append(f"{rel['type']}→{names}")
        if "text" in n:
            parts.append("text=" + json.dumps(n["text"], ensure_ascii=False))
        ifaces = [i for i in n["interfaces"] if i not in ("Component", "Collection")]
        if ifaces:
            parts.append("[" + ",".join(ifaces) + "]")
        flags = [s for s in n["states"] if s in ("focusable", "checked", "selected", "focused")]
        if flags:
            parts.append("(" + ",".join(flags) + ")")
        yield "  " * depth + " ".join(parts)
        yield from text_lines(n.get("children", []), by_id, depth + 1)


def index(nodes, by_id):
    for n in nodes:
        by_id[n["id"]] = n
        index(n.get("children", []), by_id)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("app")
    p.add_argument("out")
    p.add_argument("--text")
    p.add_argument("--wait", type=float, default=30)
    p.add_argument("--press", nargs=3, action="append", default=[], metavar=("ROLE", "NAME", "KEY"))
    p.add_argument("--back", action="store_true", help="reach the pressed elements with Shift+Tab")
    args = p.parse_args()
    app = find_app(args.app, args.wait)
    # Let the window finish its first layout and accessibility updates.
    time.sleep(1.5)
    for role, name, key_name in args.press:
        press(app, role, name, key_name, args.back)
    ids, tree = {}, []
    walk(app, ids, tree)
    resolve(tree, ids)
    # The GTK version the tree comes from.
    tree[0]["toolkit"] = {
        "name": safe(app.get_toolkit_name, ""),
        "version": safe(app.get_toolkit_version, ""),
    }
    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(tree, f, ensure_ascii=False, indent=1)
    if args.text:
        by_id = {}
        index(tree, by_id)
        with open(args.text, "w", encoding="utf-8") as f:
            f.write("\n".join(text_lines(tree, by_id)) + "\n")
    print(f"atspi-dump: {len(ids)} elements → {args.out}")


if __name__ == "__main__":
    main()
