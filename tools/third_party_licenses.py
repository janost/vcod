#!/usr/bin/env python3
"""Write the licence notices of every third-party crate the release binaries
link, for the nightly archives (NOTICE points at the file).

    tools/third_party_licenses.py <target-triple> <out-file>

Reads `cargo metadata --locked --filter-platform <triple>` and walks the
normal (not dev, not build) dependencies of the vcod and vcod-server
packages. metadata resolves features for the whole workspace, so the list
holds every crate `cargo tree -e normal --target <triple>` prints and a few
the binaries do not link. For each crate it copies the licence, copying and
notice files the published package carries; a text already printed for
another crate is named, not repeated.
"""
import hashlib
import json
import os
import subprocess
import sys

ROOTS = ("vcod", "vcod-server")
PREFIXES = ("license", "licence", "copying", "notice", "unlicense", "copyright")


def main():
    triple, out_path = sys.argv[1], sys.argv[2]
    meta = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", triple],
            check=True,
            capture_output=True,
        ).stdout
    )
    pkgs = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    members = set(meta["workspace_members"])

    seen, stack = set(), [i for i in members if pkgs[i]["name"] in ROOTS]
    while stack:
        node = nodes[stack.pop()]
        for dep in node["deps"]:
            if any(k["kind"] is None for k in dep["dep_kinds"]) and dep["pkg"] not in seen:
                seen.add(dep["pkg"])
                stack.append(dep["pkg"])
    crates = sorted((pkgs[i] for i in seen - members), key=lambda p: (p["name"], p["version"]))

    texts = {}
    with open(out_path, "w", encoding="utf-8", newline="\n") as out:
        out.write(
            "Third-party licences\n\n"
            "vcod and vcod-server link the crates below, built from their published\n"
            "sources on crates.io (https://crates.io/crates/<name>/<version>), which\n"
            "is also where the source of each MPL-2.0 crate is available. Each entry\n"
            "gives the crate's licence expression and the licence and notice files\n"
            f"its package carries. Target: {triple}. {len(crates)} crates.\n"
        )
        for p in crates:
            out.write("\n" + "=" * 78 + "\n")
            out.write(f"{p['name']} {p['version']}\n")
            out.write(f"Licence: {p.get('license') or 'see the file below'}\n")
            if p.get("repository"):
                out.write(f"Source: {p['repository']}\n")
            root = os.path.dirname(p["manifest_path"])
            files = sorted(
                f
                for f in os.listdir(root)
                if f.lower().startswith(PREFIXES) and os.path.isfile(os.path.join(root, f))
            )
            if p.get("license_file") and os.path.basename(p["license_file"]) not in files:
                files.append(p["license_file"])
            if not files:
                out.write("The package carries no licence file; its licence is the expression above.\n")
            for f in files:
                with open(os.path.join(root, f), encoding="utf-8", errors="replace") as fh:
                    text = fh.read().strip("\n")
                key = hashlib.sha256(text.encode()).hexdigest()
                if key in texts:
                    out.write(f"\n{f}: the same text as {texts[key]}.\n")
                    continue
                texts[key] = f"{p['name']} {p['version']}'s {f}"
                out.write(f"\n--- {f} ---\n{text}\n")


if __name__ == "__main__":
    main()
