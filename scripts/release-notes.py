#!/usr/bin/env python3
"""Prints the GitHub release notes of a version from CHANGELOG.md.

Usage: scripts/release-notes.py 0.1.0 [CHANGELOG.md]

The notes are the version's section (`## 0.1.0 (date)`) without its
heading. GitHub takes 125,000 characters at most: a longer section gives
its summary (the text before its first `###`) and a link to the full list
in CHANGELOG.md at the version's tag.
"""

import re
import sys

LIMIT = 120_000
REPO = "https://github.com/0xtlt/prismsdk"


def unwrap(text: str) -> str:
    """Joins hard-wrapped lines: GitHub shows each newline of release notes
    as a line break. Code blocks, tables and headings stay as they are."""
    out: list[str] = []
    code = False
    for line in text.split("\n"):
        stripped = line.strip()
        if stripped.startswith("```"):
            code = not code
            out.append(line)
            continue
        joins = (
            not code
            and out
            and out[-1].strip()
            and not out[-1].lstrip().startswith(("```", "|", "#"))
            and stripped
            and not stripped.startswith(("- ", "* ", "|", "#", "```"))
            and not re.match(r"\d+\. ", stripped)
        )
        if joins:
            out[-1] = out[-1].rstrip() + " " + stripped
        else:
            out.append(line)
    return "\n".join(out)


def notes(version: str, changelog: str) -> str:
    heading = re.compile(rf"^## {re.escape(version)}(\s|$)", re.M)
    start = heading.search(changelog)
    if not start:
        raise SystemExit(f"CHANGELOG.md has no section for {version}")
    body = changelog[start.end():]
    following = re.search(r"^## ", body, re.M)
    if following:
        body = body[: following.start()]
    body = body.split("\n", 1)[1] if "\n" in body else ""
    body = unwrap(body.strip()) + "\n"
    if len(body) <= LIMIT:
        return body
    # Too long: the summary, then each entry's first sentence by section.
    parts = re.split(r"^### (.+)$", body, flags=re.M)
    out = [parts[0].strip(), ""]
    for title, section in zip(parts[1::2], parts[2::2]):
        out += [f"### {title.strip()}", ""]
        for line in section.split("\n"):
            if line.startswith("- "):
                out.append("- " + first_sentence(line[2:]))
            elif line.strip() and not out[-1].startswith("- "):
                out.append(line)
        out.append("")
    out.append(
        f"Each entry in full: [CHANGELOG.md]({REPO}/blob/v{version}/CHANGELOG.md)."
    )
    short = "\n".join(out) + "\n"
    if len(short) <= LIMIT:
        return short
    return (
        f"{parts[0].strip()}\n\nThe full list of changes is in "
        f"[CHANGELOG.md]({REPO}/blob/v{version}/CHANGELOG.md).\n"
    )


def first_sentence(entry: str, most: int = 280) -> str:
    """An entry's first sentence (up to `. ` outside code), at most `most`
    characters."""
    code = False
    for i, ch in enumerate(entry):
        if ch == "`":
            code = not code
        elif not code and entry.startswith(". ", i) and i > 10:
            entry = entry[: i + 1]
            break
    if len(entry) > most:
        cut = entry.rfind(" ", 0, most)
        entry = entry[: cut if cut > 0 else most].rstrip(",;:")
        # A cut inside a code span closes it.
        if entry.count("`") % 2:
            entry += "`"
        entry += " …"
    return entry


def main() -> None:
    if len(sys.argv) not in (2, 3):
        raise SystemExit(__doc__)
    path = sys.argv[2] if len(sys.argv) == 3 else "CHANGELOG.md"
    with open(path, encoding="utf-8") as f:
        sys.stdout.write(notes(sys.argv[1], f.read()))


if __name__ == "__main__":
    main()
