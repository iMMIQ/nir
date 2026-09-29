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
roots = [p["id"] for p in packages.values() if p["name"] in {"player-web", "player-windows", "player-linux", "novelc"}]
seen = set()
def visit(key):
    if key in seen:
        return
    seen.add(key)
    for dep in nodes[key]["deps"]:
        visit(dep["pkg"])
for root in roots:
    visit(root)
# Notices are digested into the SDK identity, so they must be byte-identical
# across build hosts; Windows defaults to cp1252 for text mode otherwise.
chunks = ["NIR third-party notices. Versions are taken from Cargo.lock.\n", *[Path(name).read_text(encoding="utf-8") for name in ("LICENSE-NOTICE.md", "LICENSE", "COPYING")]]
# Some crates (symphonia 0.5.x) declare MPL-2.0 but publish no license file;
# the license is one canonical document, bundled here so notices stay
# complete without network access during the build.
MPL_2_0 = (Path(__file__).parent / "notices" / "MPL-2.0.txt").read_text(encoding="utf-8")
for p in sorted((packages[key] for key in seen), key=lambda p: (p["name"], p["version"])):
    chunks.append(f'\n=== {p["name"]} {p["version"]} — {p.get("license") or "workspace"} ===\n')
    directory = Path(p["manifest_path"]).parent
    paths = sorted(path for path in directory.iterdir() if path.is_file() and path.name.lower().startswith(("license", "copying", "notice")))
    if p.get("license_file"):
        extra = directory / p["license_file"]
        if extra not in paths:
            paths.append(extra)
    # Vendored-notice special cases: these crates build third-party sources
    # whose own notices live inside the vendored tree, not at the crate root.
    for extra in {
        "hb-subset": ("harfbuzz/COPYING",),
        "libwebp-sys": ("vendor/COPYING", "vendor/PATENTS"),
        "mp3lame-sys": ("lame-3.100/COPYING", "lame-3.100/LICENSE"),
    }.get(p["name"], ()):
        candidate = directory / extra
        if candidate.is_file() and candidate not in paths:
            paths.append(candidate)
    bodies = [path.read_text(encoding="utf-8", errors="replace") for path in paths]
    if not bodies and p.get("license") == "MPL-2.0":
        bodies.append(MPL_2_0)
    chunks.extend(bodies)
Path(sys.argv[1]).write_text("\n".join(chunks), encoding="utf-8", newline="\n")
print(f"Collected {len(seen)} package notices")
