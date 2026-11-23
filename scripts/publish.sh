#!/bin/sh
# Publishes the workspace's version of every crate not on crates.io yet, in
# dependency order (cargo orders them): run again after a failure, it picks
# up where it stopped. Verified beforehand (`cargo publish --workspace
# --dry-run`), so it uploads without building again. Needs
# CARGO_REGISTRY_TOKEN (the release workflow's, or a maintainer's).
set -eu
cd "$(dirname "$0")/.."

version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["version"])')
crates=$(cargo metadata --no-deps --format-version 1 | python3 -c '
import json, sys
for p in json.load(sys.stdin)["packages"]:
    if p.get("publish") is None or p["publish"]:
        print(p["name"])
')

# A version is on crates.io once its line is in the sparse index:
# index.crates.io/pr/is/prismsdk-core for names of four letters or more.
published() {
    name=$1
    path="$(printf %s "$name" | cut -c1-2)/$(printf %s "$name" | cut -c3-4)/$name"
    curl -sf "https://index.crates.io/$path" | grep -q "\"vers\":\"$version\""
}

args=""
for c in $crates; do
    if published "$c"; then
        echo "$c $version: on crates.io already"
    else
        args="$args -p $c"
    fi
done
if [ -z "$args" ]; then
    echo "every crate of $version is on crates.io"
    exit 0
fi
# shellcheck disable=SC2086
cargo publish --no-verify --locked $args
