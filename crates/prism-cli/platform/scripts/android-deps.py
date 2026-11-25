#!/usr/bin/env python3
"""Resolves Android libraries from Maven without Gradle (used by android-run.sh).

    android-deps.py <cache dir> group:artifact:version ...
    android-deps.py --lock <cache dir> group:artifact:version ...

Fetches each library and its compile/runtime dependencies (POMs) from Google's
Maven and Maven Central, the highest version winning, into the cache. AARs are
unpacked. Version constraints are applied as Gradle applies them: those of the
libraries' Gradle module metadata (`.module`: androidx.lifecycle's artifacts at
one version), and the Kotlin Gradle plugin's (kotlin-stdlib-jdk7/jdk8 at
kotlin-stdlib's version, their classes being in it since 1.8); otherwise the
same classes would come twice. Prints JSON: [{"id", "jars": [...], "res": dir|null, "package": str|null}].

Every file is checked against android-deps.lock (next to this script): the
SHA-256 of each file the resolution reads, by its path in the repository.
A file the lock does not list counts as absent, as it was when the lock was
made, so the same roots always resolve to the same files; a file that differs
from its digest, downloaded or cached, stops the build. `--lock` resolves from
the network instead and adds what it read to the lock (review the change).

Environment (explicit overrides):
  PRISM_MAVEN_LOCK   another lock file
  PRISM_MAVEN_REPOS  repositories to use instead, space-separated URLs
"""
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import xml.etree.ElementTree as ET
import zipfile

REPOS = os.environ.get("PRISM_MAVEN_REPOS", "").split() or [
    "https://dl.google.com/android/maven2",
    "https://repo1.maven.org/maven2",
]
NS = {"m": "http://maven.apache.org/POM/4.0.0"}
LOCK = os.environ.get("PRISM_MAVEN_LOCK") or os.path.join(
    os.path.dirname(os.path.abspath(__file__)), "android-deps.lock"
)

args = sys.argv[1:]
locking = bool(args) and args[0] == "--lock"
if locking:
    args = args[1:]
if not args:
    sys.exit(__doc__)
cache, specs = args[0], args[1:]


def fail(message):
    sys.exit(f"android-deps: {message}")


def read_lock():
    """{path: sha256} from the lock (`<sha256>  <path>` lines)."""
    lock = {}
    try:
        lines = open(LOCK, encoding="utf-8").read().splitlines()
    except FileNotFoundError:
        if locking:
            return lock
        fail(f"no lock file {LOCK}")
    for line in lines:
        if line.strip() and not line.startswith("#"):
            digest, path = line.split(None, 1)
            lock[path.strip()] = digest
    return lock


lock = read_lock()


def sha256(data):
    return hashlib.sha256(data).hexdigest()


# Group, artifact and version: they become cache paths.
PART = re.compile(r"[A-Za-z0-9_][A-Za-z0-9_.+-]*")


def coordinate(g, a, v):
    for part in (g, a, v):
        if not PART.fullmatch(part) or ".." in part:
            fail(f"invalid Maven coordinate {g}:{a}:{v}")
    return g, a, v


def write_atomic(path, data):
    """Writes `path` through a private temporary file beside it."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=os.path.dirname(path), prefix=".part-")
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(data)
        os.replace(tmp, path)
    except BaseException:
        os.unlink(tmp)
        raise


def fetch(path):
    """Bytes of `path` (cached, else from the first repository that has it), or
    None when the lock does not list it."""
    want = lock.get(path)
    if want is None and not locking:
        return None
    local = os.path.join(cache, "files", path)
    if os.path.exists(local):
        data = open(local, "rb").read()
        if want is None or sha256(data) == want:
            if locking:
                lock[path] = sha256(data)
            return data
        fail(f"{local} does not match {os.path.basename(LOCK)} (delete it to download it again)")
    data = next((d for d in (download(f"{repo}/{path}") for repo in REPOS) if d is not None), None)
    if data is None:
        if locking:
            return None
        fail(f"{path} is in {os.path.basename(LOCK)} but no repository has it")
    got = sha256(data)
    if want is not None and got != want:
        fail(f"{path}: SHA-256 {got}, {os.path.basename(LOCK)} has {want}")
    lock[path] = got
    write_atomic(local, data)
    return data


# Waits before each new try of a request the server failed or that did not
# reach it (Maven Central answers 429 and 5xx under load), unless the answer
# says how long (Retry-After).
RETRY_DELAYS = (1, 3, 9)


def download(url):
    """The body of `url`, or None when the repository doesn't have it (404,
    410). Any other answer or a network error says nothing about the file: it
    is tried again, then stops the build. Python's TLS setup may lack the
    system certificates: curl then does it."""
    if not url.startswith(("http:", "https:")):
        try:
            with urllib.request.urlopen(url, timeout=60) as r:
                return r.read()
        except OSError:
            return None  # a local mirror (file:) without the file
    for n in range(1 + len(RETRY_DELAYS)):
        wait = None
        try:
            with urllib.request.urlopen(url, timeout=60) as r:
                return r.read()
        except urllib.error.HTTPError as e:
            if e.code in (404, 410):
                return None
            error, wait = f"HTTP {e.code}", retry_after(e.headers.get("Retry-After"))
        except Exception as e:
            got = curl(url)
            if got is None:
                error = str(getattr(e, "reason", e))
            elif got[0] == 200:
                return got[1]
            elif got[0] in (404, 410):
                return None
            else:
                error = f"HTTP {got[0]}"
        if n < len(RETRY_DELAYS):
            time.sleep(RETRY_DELAYS[n] if wait is None else wait)
    fail(f"{url}: {error} ({1 + len(RETRY_DELAYS)} tries)")


def retry_after(value):
    """The seconds a Retry-After header asks to wait (at most 30), or None."""
    value = (value or "").strip()
    return min(int(value), 30) if value.isdigit() else None


def curl(url):
    """(status, body) of `url` through curl, or None when curl got no answer."""
    with tempfile.NamedTemporaryFile() as body:
        r = subprocess.run(
            ["curl", "-sSL", "--max-time", "120", "-o", body.name, "-w", "%{http_code}", url],
            capture_output=True,
            text=True,
        )
        status = r.stdout.strip()
        if r.returncode != 0 or not status.isdigit() or status == "000":
            return None
        return int(status), open(body.name, "rb").read()


def key(v):
    return [int(x) if x.isdigit() else -1 for x in re.split(r"[.-]", v)]


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
        out.append(coordinate(text(d, "groupId"), text(d, "artifactId"), version))
    return out, packaging


def module_constraints(g, a, v):
    """(group, artifact, version) the library's Gradle module metadata
    constrains: versions its siblings must have at least, if they are used."""
    data = fetch(f"{g.replace('.', '/')}/{a}/{v}/{a}-{v}.module")
    if not data:
        return []
    try:
        meta = json.loads(data)
    except ValueError:
        return []
    out = []
    for variant in meta.get("variants", []):
        for c in variant.get("dependencyConstraints", []):
            version = c.get("version", {})
            want = version.get("strictly") or version.get("requires")
            if c.get("group") and c.get("module") and want and "[" not in want and "(" not in want:
                out.append(coordinate(c["group"], c["module"], want))
    return out


# The Kotlin Gradle plugin's alignment: since Kotlin 1.8 the classes of
# kotlin-stdlib-jdk7/jdk8 are in kotlin-stdlib, and those artifacts are empty.
KOTLIN_ALIGNED = {("org.jetbrains.kotlin", "kotlin-stdlib"): ["kotlin-stdlib-jdk7", "kotlin-stdlib-jdk8"]}


def constraints_of(g, a, v):
    out = module_constraints(g, a, v)
    if key(v) >= key("1.8.0"):
        out += [(g, sibling, v) for sibling in KOTLIN_ALIGNED.get((g, a), [])]
    return out


# Breadth first; the highest version of each library wins, and constraints
# raise the versions of the libraries they name (those used).
chosen, packaging, queue, floors = {}, {}, [], {}
for spec in specs:
    parts = spec.split(":")
    if len(parts) != 3:
        fail(f"{spec}: group:artifact:version (versions are pinned)")
    root = coordinate(*parts)
    if not locking and pom_path(*root) not in lock:
        fail(f"{spec} is not in {os.path.basename(LOCK)} (add it with --lock)")
    queue.append(root)
while queue:
    g, a, v = queue.pop(0)
    floor = floors.get((g, a))
    if floor and key(floor) > key(v):
        v = floor
    current = chosen.get((g, a))
    if current and key(current) >= key(v):
        continue
    chosen[(g, a)] = v
    children, packaging[(g, a)] = deps(g, a, v)
    queue.extend(children)
    for cg, ca, cv in constraints_of(g, a, v):
        if key(cv) > key(floors.get((cg, ca), "0")):
            floors[(cg, ca)] = cv
            if (cg, ca) in chosen and key(chosen[(cg, ca)]) < key(cv):
                queue.append((cg, ca, cv))

result = []
unpacked = os.path.join(cache, "unpacked")
for (g, a), v in sorted(chosen.items()):
    # Kotlin Multiplatform libraries publish -android and -jvm variants of the
    # same classes; Gradle's module metadata picks Android's: so do we.
    if a.endswith("-jvm") and (g, a[:-4] + "-android") in chosen:
        continue
    base = f"{g.replace('.', '/')}/{a}/{v}/{a}-{v}"
    kind = "aar" if packaging[(g, a)] == "aar" else "jar"
    path = f"{base}.{kind}"
    data = fetch(path)
    if data is None:
        path = f"{base}.{'jar' if kind == 'aar' else 'aar'}"
        data = fetch(path)
    if data is None:
        continue  # POM-only (BOMs, relocations)
    entry = {"id": f"{g}:{a}:{v}", "jars": [], "res": None, "package": None}
    if data[:2] == b"PK":
        # The verified archive, under the name its contents call for.
        archive = os.path.join(cache, "archives", f"{base}.{'aar' if b'AndroidManifest.xml' in data else 'jar'}")
        if not os.path.exists(archive) or open(archive, "rb").read() != data:
            write_atomic(archive, data)
        with zipfile.ZipFile(archive) as z:
            names = z.namelist()
            if "AndroidManifest.xml" in names:  # an AAR
                # Unpacked again from the verified archive on every run, in a
                # private directory, then moved in place.
                out_dir = os.path.join(unpacked, f"{g}_{a}_{v}")
                os.makedirs(unpacked, exist_ok=True)
                tmp = tempfile.mkdtemp(dir=unpacked, prefix=".part-")
                z.extractall(tmp)
                old = tempfile.mkdtemp(dir=unpacked, prefix=".old-")
                if os.path.isdir(out_dir):
                    os.replace(out_dir, os.path.join(old, "x"))
                os.replace(tmp, out_dir)
                shutil.rmtree(old, ignore_errors=True)
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

if locking:
    lines = [f"{digest}  {path}" for path, digest in sorted(lock.items())]
    header = (
        "# SHA-256 of every Maven file android-deps.py may read, by repository path.\n"
        "# Made by `android-deps.py --lock` for each set of roots android-run.sh uses.\n"
    )
    write_atomic(os.path.abspath(LOCK), (header + "\n".join(lines) + "\n").encode())
    os.chmod(LOCK, 0o644)
print(json.dumps(result, indent=1))
