#!/usr/bin/env python3
"""Smoke-test helpers (standard library only).

  check.py png FILE [--min-size W H]
      Fails unless FILE is a PNG of at least W×H pixels that is not blank:
      several distinct colors, and no single color covering ~all of it.

  check.py a11y FILE [--label TEXT]... [--button TEXT]... [--text TEXT]...
                     [--segments A,B,C] [--selected N]
      Checks a `PRISM_A11Y_DUMP` tree (macOS, iOS): each --label is some view's
      accessibility label, button title or text; each --button is a view with
      the AXButton role (or an NSButton) with that label/title; each --text
      is some label's text; --segments are the tab bar's items (macOS segmented
      control, iOS UITabBar) and
      --selected its selected index.
"""

import json
import struct
import sys
import zlib


def fail(msg):
    print(f"smoke: FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def decode_png(path):
    """(width, height, rows) with rows as lists of pixel tuples."""
    data = open(path, "rb").read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        fail(f"{path} is not a PNG")
    pos, idat, ihdr, palette = 8, b"", None, None
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        kind = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if kind == b"IHDR":
            ihdr = struct.unpack(">IIBBBBB", body)
        elif kind == b"PLTE":
            palette = [tuple(body[i : i + 3]) for i in range(0, len(body), 3)]
        elif kind == b"IDAT":
            idat += body
        elif kind == b"IEND":
            break
    if ihdr is None:
        fail(f"{path}: no IHDR")
    width, height, depth, ctype, _, _, interlace = ihdr
    if interlace:
        fail(f"{path}: interlaced PNGs are not supported")
    channels = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[ctype]
    bpp = max(1, channels * depth // 8)
    stride = (width * channels * depth + 7) // 8
    raw = zlib.decompress(idat)
    rows, prev, i = [], bytearray(stride), 0
    for _ in range(height):
        filt = raw[i]
        line = bytearray(raw[i + 1 : i + 1 + stride])
        i += 1 + stride
        for x in range(stride):
            a = line[x - bpp] if x >= bpp else 0
            b = prev[x]
            c = prev[x - bpp] if x >= bpp else 0
            if filt == 1:
                line[x] = (line[x] + a) & 0xFF
            elif filt == 2:
                line[x] = (line[x] + b) & 0xFF
            elif filt == 3:
                line[x] = (line[x] + (a + b) // 2) & 0xFF
            elif filt == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pred = a if pa <= pb and pa <= pc else (b if pb <= pc else c)
                line[x] = (line[x] + pred) & 0xFF
        rows.append(line)
        prev = line
    step = channels * depth // 8 if depth >= 8 else 1
    pixels = []
    for line in rows:
        if depth < 8:
            pixels.append([(v,) for v in line])
        else:
            px = [tuple(line[x : x + step]) for x in range(0, len(line), step)]
            if ctype == 3 and palette:
                px = [palette[p[0]] if p[0] < len(palette) else p for p in px]
            pixels.append(px)
    return width, height, pixels


def check_png(args):
    path = args[0]
    min_w, min_h = 100, 100
    if "--min-size" in args:
        k = args.index("--min-size")
        min_w, min_h = int(args[k + 1]), int(args[k + 2])
    width, height, rows = decode_png(path)
    if width < min_w or height < min_h:
        fail(f"{path}: {width}×{height} is smaller than {min_w}×{min_h}")
    counts = {}
    total = 0
    for y in range(0, height, max(1, height // 200)):
        for px in rows[y][:: max(1, width // 200)]:
            counts[px] = counts.get(px, 0) + 1
            total += 1
    top = max(counts.values())
    if len(counts) < 8:
        fail(f"{path}: only {len(counts)} distinct colors: blank or broken render")
    if top / total > 0.97:
        fail(f"{path}: one color covers {100 * top / total:.1f}% of the image")
    print(
        f"smoke: {path}: {width}×{height}, {len(counts)} colors, "
        f"dominant {100 * top / total:.0f}%"
    )


def walk(node):
    yield node
    for child in node.get("children", []):
        yield from walk(child)


def check_a11y(args):
    tree = json.load(open(args[0]))
    nodes = list(walk(tree["root"]))
    for sheet in tree.get("sheets", []):
        nodes += list(walk(sheet))

    def names(n):
        return {v for v in (n.get("label"), n.get("title"), n.get("value")) if v}

    i = 1
    while i < len(args):
        flag, value = args[i], args[i + 1]
        i += 2
        if flag == "--label":
            if not any(value in names(n) for n in nodes):
                fail(f"no view labelled {value!r}")
        elif flag == "--text":
            if not any(value == n.get("value") for n in nodes):
                fail(f"no text {value!r}")
        elif flag == "--button":
            ok = any(
                (n.get("role") == "AXButton" or n.get("class", "").endswith("NSButton"))
                and value in names(n)
                for n in nodes
            )
            if not ok:
                fail(f"no button {value!r}")
        elif flag in ("--segments", "--selected"):
            bars = [n["segments"] for n in nodes if n.get("segments")]
            if not bars:
                fail("no tab bar (segmented control)")
            if flag == "--segments" and bars[0]["labels"] != value.split(","):
                fail(f"tab bar shows {bars[0]['labels']}, expected {value.split(',')}")
            if flag == "--selected" and bars[0]["selected"] != int(value):
                fail(f"tab {bars[0]['selected']} is selected, expected {value}")
        else:
            fail(f"unknown flag {flag}")
    print(f"smoke: {args[0]}: {len(nodes)} views, expectations met")


if __name__ == "__main__":
    if len(sys.argv) < 3 or sys.argv[1] not in ("png", "a11y"):
        print(__doc__)
        sys.exit(2)
    {"png": check_png, "a11y": check_a11y}[sys.argv[1]](sys.argv[2:])
