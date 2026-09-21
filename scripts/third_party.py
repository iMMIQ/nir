#!/usr/bin/env python3
"""Collect notices for both distributed programs from the locked dependency graph."""
import json
from pathlib import Path
import subprocess
import sys

metadata = json.loads(subprocess.check_output([
    "cargo", "metadata", "--locked", "--format-version", "1",
]))
packages = {p["id"]: p for p in metadata["packages"]}
nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
roots = [p["id"] for p in packages.values() if p["name"] in {"player-web", "novelc"}]
seen = set()
def visit(key):
    if key in seen:
        return
    seen.add(key)
    for dep in nodes[key]["deps"]:
        visit(dep["pkg"])
for root in roots:
    visit(root)
chunks = ["NIR third-party notices. Versions are taken from Cargo.lock.\n", *[Path(name).read_text() for name in ("LICENSE-NOTICE.md", "LICENSE", "COPYING")]]
for p in sorted((packages[key] for key in seen), key=lambda p: (p["name"], p["version"])):
    chunks.append(f'\n=== {p["name"]} {p["version"]} — {p.get("license") or "workspace"} ===\n')
    directory = Path(p["manifest_path"]).parent
    paths = sorted(path for path in directory.iterdir() if path.is_file() and path.name.lower().startswith(("license", "copying", "notice")))
    if p.get("license_file"):
        extra = directory / p["license_file"]
        if extra not in paths:
            paths.append(extra)
    for path in paths:
        chunks.append(path.read_text(errors="replace"))
Path(sys.argv[1]).write_text("\n".join(chunks))
print(f"Collected {len(seen)} package notices")
