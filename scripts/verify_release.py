#!/usr/bin/env python3
"""Validate every object of a final static release, independently of Rust/HTTP."""
import hashlib
import json
from pathlib import Path
import sys

root = Path(sys.argv[1]).resolve()
channel = json.loads((root / "channels/stable.json").read_bytes())
assert channel["format"] == 1
release_id = channel["release"]
assert len(release_id) == 64 and all(c in "0123456789abcdef" for c in release_id)
raw = (root / "releases" / f"{release_id}.json").read_bytes()
assert hashlib.sha256(raw).hexdigest() == release_id, "release digest mismatch"
release = json.loads(raw)
total = 0
for identity, obj in release["objects"].items():
    path = (root / obj["path"]).resolve()
    assert path.is_relative_to(root / "objects"), "escaped object root"
    data = path.read_bytes()
    assert len(data) == obj["bytes"], f"size mismatch: {identity}"
    assert hashlib.sha256(data).hexdigest() == identity, f"digest mismatch: {identity}"
    total += len(data)
for identity in [release["program"], *release["engine"].values(), *release["notices"]]:
    assert identity in release["objects"], "missing release root"
wasm = release["objects"][release["engine"]["wasm"]]
assert (root / wasm["path"]).read_bytes().startswith(b"\0asm\x01\0\0\0"), "not actual WASM"
program = json.loads((root / release["objects"][release["program"]]["path"]).read_bytes())
for asset in program["program"]["assets"].values():
    assert asset["object"] in release["objects"], "missing asset"
assert (root / "NOTICE.txt").read_text().find("SIL OPEN FONT LICENSE") >= 0
assert not any((root / path).exists() for path in ["game.toml", "game.lock", "content", "tests", "assets/source"])
print(json.dumps({"release": release_id, "objects": len(release["objects"]), "bytes": total, "status": "PASS"}, indent=2))
