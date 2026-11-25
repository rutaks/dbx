#!/usr/bin/env python3
"""Appends to legal/THIRD-PARTY.txt the crates that ship no licence file.

Reads the generated list on stdin (the "name version | SPDX | repository"
lines), looks each crate up in `cargo metadata` (given as the first argument,
a JSON file), and prints a section naming every crate whose package holds no
LICENSE, COPYING, UNLICENSE or NOTICE file, with the SPDX expression it
declares. Crates of the dbx workspace itself (no registry source) are
Apache-2.0 under dbx's LICENSE, which ships beside this file.
"""
import json
import os
import re
import sys

LICENCE_FILE = re.compile(r"(?i)^(licen[cs]e|copying|unlicense|notice)")


def main() -> None:
    meta = json.load(open(sys.argv[1]))
    packages = {(p["name"], p["version"]): p for p in meta["packages"]}
    listed = set()
    for line in sys.stdin:
        m = re.match(r"^([A-Za-z0-9_.-]+) ([0-9][^ ]*) \| ", line)
        if m:
            listed.add((m[1], m[2]))
    workspace, bare = [], []
    for key in sorted(listed):
        pkg = packages.get(key)
        if pkg is None:
            continue
        directory = os.path.dirname(pkg["manifest_path"])
        if any(LICENCE_FILE.match(f) for f in os.listdir(directory)):
            continue
        entry = (key[0], key[1], pkg.get("license") or "(none declared)", pkg.get("repository") or "")
        (bare if pkg.get("source") else workspace).append(entry)
    print("Crates that ship no licence file")
    print()
    print("The packages of these crates hold no licence text or copyright notice, so")
    print("none can be reproduced above. Each is listed with the SPDX expression its")
    print("manifest declares; that expression is the licence it is used under, and the")
    print("repository is where its copyright holders are named.")
    print()
    for name, version, spdx, repo in bare:
        print(f"{name} {version} | {spdx}" + (f" | {repo}" if repo else ""))
    print()
    print("Crates of the dbx workspace (Apache-2.0, covered by dbx's LICENSE, which")
    print("ships beside this file): " + ", ".join(f"{n} {v}" for n, v, _, _ in workspace))


main()
