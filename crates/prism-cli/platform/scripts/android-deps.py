#!/usr/bin/env python3
"""Resolves Android libraries from Maven without Gradle (used by android-run.sh).

    android-deps.py <cache dir> group:artifact[:version] ...

Fetches each library and its compile/runtime dependencies (POMs) from Google's
Maven and Maven Central, the highest version winning, into the cache. AARs are
unpacked. Prints JSON: [{"id", "jars": [...], "res": dir|null, "package": str|null}].
A missing version means the latest stable release.
"""
import json
import os
import re
import sys
import subprocess
import urllib.error
import urllib.request
import xml.etree.ElementTree as ET
import zipfile

REPOS = ["https://dl.google.com/android/maven2", "https://repo1.maven.org/maven2"]
NS = {"m": "http://maven.apache.org/POM/4.0.0"}
cache = sys.argv[1]
os.makedirs(cache, exist_ok=True)


def fetch(path):
    """Bytes of `path` from the first repository that has it (cached), or None."""
    local = os.path.join(cache, "files", path)
    if os.path.exists(local):
        return open(local, "rb").read()
    if os.path.exists(local + ".missing"):
        return None
    for repo in REPOS:
        data = download(f"{repo}/{path}")
        if data is not None:
            os.makedirs(os.path.dirname(local), exist_ok=True)
            open(local, "wb").write(data)
            return data
    os.makedirs(os.path.dirname(local), exist_ok=True)
    open(local + ".missing", "w").close()
    return None


def download(url):
    """The body of `url`, or None on 404. Python's TLS setup may lack the system
    certificates: curl then does it."""
    try:
        with urllib.request.urlopen(url, timeout=60) as r:
            return r.read()
    except urllib.error.HTTPError:
        return None
    except Exception:
        r = subprocess.run(["curl", "-fsSL", "--max-time", "120", url], capture_output=True)
        return r.stdout if r.returncode == 0 else None


def key(v):
    return [int(x) if x.isdigit() else -1 for x in re.split(r"[.-]", v)]


def stable(v):
    return not re.search(r"alpha|beta|rc|dev|snapshot|eap|m\d", v, re.I)


def latest(group, artifact):
    data = fetch(f"{group.replace('.', '/')}/{artifact}/maven-metadata.xml")
    if not data:
        sys.exit(f"android-deps: {group}:{artifact} not found")
    versions = [v.text for v in ET.fromstring(data).iter("version")]
    return max((v for v in versions if stable(v)), key=key, default=versions[-1])


def pom_path(g, a, v):
    return f"{g.replace('.', '/')}/{a}/{v}/{a}-{v}.pom"


def deps(g, a, v):
    """(group, artifact, version) this library needs at runtime, and its packaging."""
    data = fetch(pom_path(g, a, v))
    if not data:
        return [], "jar"
    root = ET.fromstring(data)
    props = {"project.version": v, "pom.version": v}
    for p in root.findall("m:properties/*", NS):
        props[p.tag.split("}")[1]] = (p.text or "").strip()
    def text(el, tag):
        t = el.find(f"m:{tag}", NS)
        s = (t.text or "").strip() if t is not None else ""
        return re.sub(r"\$\{([^}]+)\}", lambda m: props.get(m.group(1), m.group(0)), s)
    packaging = text(root, "packaging") or "jar"
    out = []
    for d in root.findall("m:dependencies/m:dependency", NS):
        if text(d, "scope") in ("test", "provided", "system") or text(d, "optional") == "true":
            continue
        version = text(d, "version").strip("[]()").split(",")[0]
        if not version or "${" in version:
            continue
        out.append((text(d, "groupId"), text(d, "artifactId"), version))
    return out, packaging


# Breadth first; the highest version of each library wins.
chosen, packaging, queue = {}, {}, []
for spec in sys.argv[2:]:
    parts = spec.split(":")
    g, a = parts[0], parts[1]
    queue.append((g, a, parts[2] if len(parts) > 2 else latest(g, a)))
while queue:
    g, a, v = queue.pop(0)
    current = chosen.get((g, a))
    if current and key(current) >= key(v):
        continue
    chosen[(g, a)] = v
    children, packaging[(g, a)] = deps(g, a, v)
    queue.extend(children)

result = []
for (g, a), v in sorted(chosen.items()):
    # Kotlin Multiplatform libraries publish -android and -jvm variants of the
    # same classes; Gradle's module metadata picks Android's: so do we.
    if a.endswith("-jvm") and (g, a[:-4] + "-android") in chosen:
        continue
    base = f"{g.replace('.', '/')}/{a}/{v}/{a}-{v}"
    kind = "aar" if packaging[(g, a)] == "aar" else "jar"
    data = fetch(f"{base}.{kind}") or fetch(f"{base}.{'jar' if kind == 'aar' else 'aar'}")
    if data is None:
        continue  # POM-only (BOMs, relocations)
    out_dir = os.path.join(cache, "unpacked", f"{g}_{a}_{v}")
    entry = {"id": f"{g}:{a}:{v}", "jars": [], "res": None, "package": None}
    if data[:2] == b"PK" and (kind == "aar" or b"AndroidManifest.xml" in data[:4096] or True):
        archive = os.path.join(cache, "files", f"{base}.{'aar' if b'AndroidManifest.xml' in data else 'jar'}")
        open(archive, "wb").write(data)
        with zipfile.ZipFile(archive) as z:
            names = z.namelist()
            if "AndroidManifest.xml" in names:  # an AAR
                if not os.path.isdir(out_dir):
                    z.extractall(out_dir)
                jars = [os.path.join(out_dir, "classes.jar")] if "classes.jar" in names else []
                jars += [os.path.join(out_dir, n) for n in names if n.startswith("libs/") and n.endswith(".jar")]
                entry["jars"] = jars
                res = os.path.join(out_dir, "res")
                if os.path.isdir(res) and any(os.scandir(res)):
                    entry["res"] = res
                m = re.search(rb'package="([^"]+)"', open(os.path.join(out_dir, "AndroidManifest.xml"), "rb").read())
                entry["package"] = m.group(1).decode() if m else None
            else:
                entry["jars"] = [archive]
    result.append(entry)
print(json.dumps(result, indent=1))
