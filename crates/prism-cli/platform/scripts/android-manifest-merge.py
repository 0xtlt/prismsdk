#!/usr/bin/env python3
"""Merges libraries' AndroidManifest.xml entries without Gradle (used by
android-run.sh for the Firebase SDK and Google Play Billing).

    android-manifest-merge.py <application id> <unpacked aar dir> ...

Prints the libraries' manifest-level entries (`<permission>`, `<uses-permission>`,
`<queries>`), then a line `--`, then their `<application>` children: components
with the same class are merged (their meta-data and intent filters combined,
as Gradle's merger does), `${applicationId}` is replaced, and `tools:`
attributes are dropped.
"""
import os
import sys
import xml.etree.ElementTree as ET

ANDROID = "http://schemas.android.com/apk/res/android"
TOOLS = "http://schemas.android.com/tools"
ET.register_namespace("android", ANDROID)
NAME = f"{{{ANDROID}}}name"

app_id, dirs = sys.argv[1], sys.argv[2:]
permissions, components, order = [], {}, []
# Declared permissions (androidx.core's for its not-exported receivers) and
# package visibility (Play's billing service), once each.
declared, queries = [], []


def clean(el):
    for k in list(el.attrib):
        if k.startswith(f"{{{TOOLS}}}"):
            del el.attrib[k]
        else:
            el.attrib[k] = el.attrib[k].replace("${applicationId}", app_id)
    for c in el:
        clean(c)


for d in dirs:
    path = os.path.join(d, "AndroidManifest.xml")
    if not os.path.exists(path):
        continue
    root = ET.parse(path).getroot()
    clean(root)
    for p in root.findall("uses-permission"):
        name = p.get(NAME)
        if name and name not in permissions:
            permissions.append(name)
    for el, seen in [(e, declared) for e in root.findall("permission")] + [(e, queries) for e in root.findall("queries")]:
        xml = ET.tostring(el, encoding="unicode").replace(f' xmlns:android="{ANDROID}"', "").strip()
        if xml not in seen:
            seen.append(xml)
    app = root.find("application")
    if app is None:
        continue
    for el in app:
        if el.get(f"{{{TOOLS}}}node") == "remove":
            continue
        key = (el.tag, el.get(NAME))
        if key not in components:
            components[key] = el
            order.append(key)
            continue
        # Same component in several libraries: combine children, once each.
        merged = components[key]
        seen = {ET.tostring(c) for c in merged}
        for c in el:
            if ET.tostring(c) not in seen:
                merged.append(c)
                seen.add(ET.tostring(c))

for xml in declared:
    print("  " + xml)
for p in permissions:
    print(f'  <uses-permission android:name="{p}"/>')
for xml in queries:
    print("  " + xml)
print("--")
for key in order:
    xml = ET.tostring(components[key], encoding="unicode")
    # Declared once on the manifest root.
    print("    " + xml.replace(f' xmlns:android="{ANDROID}"', ""))
