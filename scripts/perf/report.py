#!/usr/bin/env python3
"""The medians of PRISM_TRACE startup logs, as a Markdown table.

  report.py LOG...

Each log is one run of an app with PRISM_TRACE=1 (scripts/windows-perf.sh,
scripts/macos-perf.sh), named `<profile>-<run>.log`: the runs of a profile
make one column. The first run of a profile warms the disk cache and is
left out when there are more. Standard library only.

Rows:
- each startup phase the app logged (`<phase> after N ms`, since the
  process was created), with the UI thread's CPU time then when logged
  (`(UI thread CPU M ms)`): the clock depends on the machine's load, CPU
  time much less;
- where the backend's time went while building the first screen
  (`elements: patches … ms (create …); layout … ms (measure …)`);
- the memory at the first frame and at the end;
- each `PRISM_CLICK` step, from the click to the frame showing it.
"""

import os
import re
import statistics
import sys
from collections import defaultdict

NUM = r"([0-9.]+)"
PHASE = re.compile(
    r"^prism-\w+: (.+?) after " + NUM + r" ms(?: \(UI thread CPU " + NUM + r" ms\))?"
)
COSTS = ("create", "style", "children", "measure", "text layout", "place", "flush")


def parse(path):
    """[(row, value)] for one log, in the order it logged them."""
    rows = []
    seen = set()

    def add(row, value):
        if row not in seen:
            seen.add(row)
            rows.append((row, value))

    with open(path, encoding="utf-8", errors="replace") as f:
        lines = f.read().splitlines()
    steps = 0
    for line in lines:
        m = re.search(r"step (.+?): on screen after " + NUM + " ms", line)
        if m:
            steps += 1
            add(f"step {steps}: {m.group(1)}, to the screen (ms)", float(m.group(2)))
            continue
        m = PHASE.match(line)
        if m and "on screen" not in line and not line.startswith("prism-windows: step "):
            phase = re.sub(r" \(.*", "", m.group(1)).replace("the app's tree", "app's tree built")
            add(f"{phase} (ms)", float(m.group(2)))
            if m.group(3):
                add(f"{phase}, UI thread CPU (ms)", float(m.group(3)))
        m = re.search(r"the app's tree \(" + NUM + r" ms, (\d+) patches\)", line)
        if m:
            add("app's tree: build (ms)", float(m.group(1)))
            add("app's tree: patches", float(m.group(2)))
        m = re.search(r"first layout after [0-9.]+ ms.*?\((\d+) nodes\)", line)
        if m:
            add("nodes", float(m.group(1)))
        m = re.search(r"(?:elements|views): patches (?:and layout )?" + NUM + r" ms(.*)", line)
        if m:
            patches, rest = float(m.group(1)), m.group(2)
            add("backend: patches (ms)", patches)
            lm = re.search(r"; layout " + NUM + r" ms(.*)", rest)
            head = rest[: lm.start()] if lm else rest
            for c in COSTS:
                cm = re.search(r"\b" + c + r" " + NUM + r" ms \((\d+)\)", head)
                if cm:
                    add(f"backend: patches: {c} (ms)", float(cm.group(1)))
            if lm:
                add("backend: layout (ms)", float(lm.group(1)))
                for c in COSTS:
                    cm = re.search(r"\b" + c + r" " + NUM + r" ms \((\d+)\)", lm.group(2))
                    if cm:
                        add(f"backend: layout: {c} (ms)", float(cm.group(1)))
                        if c in ("measure", "text layout"):
                            add(f"backend: layout: {c} calls", float(cm.group(2)))
        m = re.search(r"(\d+) XAML elements", line)
        if m:
            add("XAML elements", float(m.group(1)))
        for when, pattern in (("first frame", "first frame after"), ("end", "snapshot written")):
            if pattern in line:
                for name, label in (("working set", "working set"), ("private", "private")):
                    mm = re.search(name + " " + NUM + " MB", line)
                    if mm:
                        add(f"memory at the {when}: {label} (MB)", float(mm.group(1)))
    return rows


def main(paths):
    runs = defaultdict(list)
    for p in sorted(paths):
        base = os.path.basename(p)
        m = re.match(r"(.+)-(\d+)\.log$", base)
        profile, n = (m.group(1), int(m.group(2))) if m else (base, 1)
        runs[profile].append((n, parse(p)))
    if not runs:
        print("report: no logs", file=sys.stderr)
        return 1
    order = []
    columns = {}
    for profile, rs in runs.items():
        rs.sort(key=lambda r: r[0])
        used = [v for n, v in rs if n > 1] if len(rs) > 1 else [v for _, v in rs]
        values = defaultdict(list)
        for run in used:
            for row, v in run:
                values[row].append(v)
                if row not in order:
                    order.append(row)
        columns[profile] = ({k: statistics.median(v) for k, v in values.items()}, len(used))
    # Startup phases first (in the order logged), then the rest by kind.
    def kind(row):
        for i, prefix in enumerate(("backend:", "XAML elements", "nodes", "memory", "step")):
            if row.startswith(prefix):
                return i + 1
        return 0
    order.sort(key=kind)
    profiles = sorted(columns, key=lambda p: (p != "release", p))
    print("| | " + " | ".join(f"{p} (median of {columns[p][1]})" for p in profiles) + " |")
    print("|---|" + "---:|" * len(profiles))
    for row in order:
        cells = []
        for p in profiles:
            v = columns[p][0].get(row)
            if v is None:
                cells.append("")
            elif "(MB)" in row:
                cells.append(f"{v:.1f}")
            elif v >= 10 or v == int(v):
                cells.append(f"{v:.0f}")
            else:
                cells.append(f"{v:.1f}")
        bold = row.startswith("first frame (ms)")
        label = f"**{row}**" if bold else row
        print(f"| {label} | " + " | ".join(f"**{c}**" if bold and c else c for c in cells) + " |")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
