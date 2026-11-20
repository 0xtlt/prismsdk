#!/usr/bin/env python3
"""Where the UI thread's time went, from a PRISM_PROFILE file (Windows).

  profile.py STACKS [--nm NM.txt] [--exe counter.exe] [--top 15]

STACKS: folded stacks, one line each: `<phase> mod+0xrva;...;mod+0xrva N`
(outermost frame first). NM.txt: `nm -C --defined-only counter.exe` of the
same build (with symbols: `--config profile.release.strip=false`), to name
the executable's frames; its addresses are virtual, the image base comes
off the executable's PE header (`--exe`) or defaults to 0x140000000.

For each startup phase: its samples (1 ms each), the modules the UI
thread was in (innermost frame), the executable's functions on the stack
(inclusive), and the executable's innermost function on the stack (what
called into the system: a XAML property, a text layout...). Standard
library only.
"""

import argparse
import bisect
import re
import struct
import sys
from collections import Counter, defaultdict, OrderedDict


def image_base(exe):
    with open(exe, "rb") as f:
        head = f.read(4096)
    pe = struct.unpack_from("<I", head, 0x3C)[0]
    magic = struct.unpack_from("<H", head, pe + 24)[0]
    if magic == 0x20B:  # PE32+
        return struct.unpack_from("<Q", head, pe + 24 + 24)[0]
    return struct.unpack_from("<I", head, pe + 24 + 28)[0]


def load_nm(path, base):
    addrs, names = [], []
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            m = re.match(r"([0-9a-fA-F]+) [tTwW] (.+)", line.strip())
            if m:
                addrs.append(int(m.group(1), 16) - base)
                names.append(m.group(2))
    order = sorted(range(len(addrs)), key=addrs.__getitem__)
    return [addrs[i] for i in order], [names[i] for i in order]


def short(name):
    # Drop hashes and generic noise from Rust symbols.
    name = re.sub(r"::h[0-9a-f]{16}$", "", name)
    name = re.sub(r"<impl [^>]*>", "impl", name)
    return name[:140]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("stacks")
    ap.add_argument("--nm")
    ap.add_argument("--exe")
    ap.add_argument("--base", type=lambda v: int(v, 0), default=0x140000000)
    ap.add_argument("--top", type=int, default=15)
    a = ap.parse_args()
    base = image_base(a.exe) if a.exe else a.base
    syms = load_nm(a.nm, base) if a.nm else ([], [])
    exe_name = None

    def name(frame):
        nonlocal exe_name
        mod, _, rva = frame.rpartition("+")
        if mod.lower().endswith(".exe"):
            exe_name = mod
            if syms[0]:
                i = bisect.bisect_right(syms[0], int(rva, 16)) - 1
                if i >= 0:
                    return short(syms[1][i])
            return frame
        return mod

    phases = OrderedDict()
    for line in open(a.stacks, encoding="utf-8", errors="replace"):
        line = line.rstrip("\n")
        if not line:
            continue
        phase, rest = line.split(" ", 1)
        stack, _, count = rest.rpartition(" ")
        frames = stack.split(";")
        phases.setdefault(phase, []).append((frames, int(count)))

    total = sum(c for p in phases.values() for _, c in p)
    print(f"{total} samples (about {total} ms of the UI thread)\n")
    for phase, stacks in phases.items():
        n = sum(c for _, c in stacks)
        leaf_mod = Counter()
        inclusive = Counter()
        caller = Counter()
        for frames, c in stacks:
            named = [name(f) for f in frames]
            leaf_mod[frames[-1].rpartition("+")[0]] += c
            exe_frames = [nm for f, nm in zip(frames, named) if f.lower().split("+")[0].endswith(".exe")]
            for fn in set(exe_frames):
                inclusive[fn] += c
            if exe_frames:
                caller[exe_frames[-1]] += c
        print(f"## {phase.replace('_', ' ')}: {n} samples ({100 * n / max(total, 1):.0f} %)")
        print("in module (innermost frame):")
        for m, c in leaf_mod.most_common(6):
            print(f"  {c:6} {100 * c / n:5.1f} %  {m}")
        print("executable's innermost function:")
        for f, c in caller.most_common(a.top):
            print(f"  {c:6} {100 * c / n:5.1f} %  {f}")
        print("executable's functions on the stack:")
        for f, c in inclusive.most_common(a.top):
            print(f"  {c:6} {100 * c / n:5.1f} %  {f}")
        print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
