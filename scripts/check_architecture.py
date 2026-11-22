#!/usr/bin/env python3
"""Enforce NIR v2 crate layering, isolation boundaries, and single-wgpu invariant."""
import json
import subprocess

meta = json.loads(
    subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--all-features"])
)
packages = {p["id"]: p for p in meta["packages"]}
nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
workspace = set(meta["workspace_members"])

allowed = {
    # Core playback stack (strictly layered, zero compiler/importer dependencies)
    "nir-ir": set(),
    "nir-vm": {"nir-ir"},
    "nir-ui": {"nir-ir", "nir-vm"},
    "nir-render": {"nir-ir", "nir-ui"},
    "nir-runtime": {"nir-ir", "nir-vm", "nir-ui"},
    # Thin platform hosts
    "player-native": {"nir-ir", "nir-vm", "nir-ui", "nir-render", "nir-runtime"},
    "player-android": {"player-native"},
    "player-web": {"nir-ir", "nir-vm", "nir-ui", "nir-render", "nir-runtime"},
    # Build, packaging & multi-engine converter toolchain
    "nir-apk": set(),
    "nir-build": {"nir-ir", "nir-apk"},
    "nir-import-core": set(),
    "nir-import-livemaker": {"nir-import-core"},
    "nir-import-krkr": {"nir-import-core"},
    "novelc": {
        "nir-ir",
        "nir-build",
        "nir-import-core",
        "nir-import-livemaker",
        "nir-import-krkr",
    },
    "xtask": set(),
}


def edges(pkg_id):
    in_ws = pkg_id in workspace
    return [
        d["pkg"]
        for d in nodes[pkg_id]["deps"]
        if any(
            k["kind"] in (None, "build") and (in_ws or k.get("target") is None)
            for k in d["dep_kinds"]
        )
    ]


def closure(pkg_id, seen=None):
    seen = set() if seen is None else seen
    for dep in edges(pkg_id):
        if dep not in seen:
            seen.add(dep)
            closure(dep, seen)
    return seen


errors = []
for pkg_id in workspace:
    name = packages[pkg_id]["name"]
    if name not in allowed:
        errors.append(f"unknown workspace package: {name}")
        continue
    for dep in edges(pkg_id):
        if dep in workspace and packages[dep]["name"] not in allowed[name]:
            errors.append(f"forbidden workspace edge: {name} -> {packages[dep]['name']}")

    transitive = {packages[x]["name"] for x in closure(pkg_id)}
    if name in {"nir-ir", "nir-vm", "nir-ui", "nir-runtime"}:
        bad = transitive & {
            "wgpu",
            "web-sys",
            "wasm-bindgen",
            "nir-build",
            "nir-apk",
            "nir-import-core",
            "nir-import-livemaker",
            "nir-import-krkr",
        }
        if bad:
            errors.append(f"{name} leaks forbidden deps: {sorted(bad)}")
    if name in {"player-native", "player-android", "player-web"}:
        bad = transitive & {
            "nir-build",
            "nir-apk",
            "nir-import-core",
            "nir-import-livemaker",
            "nir-import-krkr",
            "hb-subset",
            "bindgen",
        }
        if bad:
            errors.append(f"player {name} links build/importer deps: {sorted(bad)}")

wgpus = {packages[x]["version"] for x in packages if packages[x]["name"] == "wgpu"}
if len(wgpus) > 1:
    errors.append(f"multiple wgpu versions in graph: {wgpus}")

if errors:
    raise SystemExit("\n".join(errors))

print(f"PASS NIR v2 dependency graph: {len(workspace)} crates, wgpu {wgpus}")
