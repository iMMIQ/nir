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
project = program["program"]
for asset in project["assets"].values():
    assert asset["object"] in release["objects"], "missing asset"

modules = project.get("modules", {})
if modules:
    assert not project["functions"], "split root unexpectedly contains module code"
    assert all(not texts for texts in project["locales"].values()), "split root unexpectedly contains module texts"
    function_owners = {}
    text_owners = {}
    for module_id, index in modules.items():
        assert index["code"] in release["objects"], f"missing code package: {module_id}"
        code = json.loads((root / release["objects"][index["code"]]["path"]).read_bytes())
        assert code["format"] == 1 and code["module"] == module_id
        assert set(code["functions"]) == set(index["functions"]), f"function index mismatch: {module_id}"
        for function_id, signature in index["functions"].items():
            assert function_id not in function_owners, f"duplicate function owner: {function_id}"
            function_owners[function_id] = module_id
            function = code["functions"][function_id]
            assert signature["params"] == function["params"]
            assert signature.get("returns") == function.get("returns")
            assert signature["entry"] == function["entry"]
            entry_ops = function["blocks"][function["entry"]]["ops"]
            entry_op = entry_ops[0]["id"] if entry_ops else "@terminator"
            assert signature["entry_op"] == entry_op, f"entry index mismatch: {function_id}"
        for text_id in index["texts"]:
            assert text_id not in text_owners, f"duplicate text owner: {text_id}"
            text_owners[text_id] = module_id
        expected_locales = set(project["locales"]) if index["texts"] else set()
        assert set(index["locales"]) == expected_locales, f"locale index mismatch: {module_id}"
        for locale, identity in index["locales"].items():
            assert identity in release["objects"], f"missing text package: {module_id}/{locale}"
            texts = json.loads((root / release["objects"][identity]["path"]).read_bytes())
            assert texts["format"] == 1 and texts["module"] == module_id and texts["locale"] == locale
            assert set(texts["texts"]) == set(index["texts"]), f"text package mismatch: {module_id}/{locale}"
    assert project["entry"] in function_owners, "entry function is not owned by a module"
    assert set(text_owners) == set(project["texts"]), "text contracts are not fully owned by modules"
else:
    # Legacy releases still carry one complete executable object.
    assert project["entry"] in project["functions"], "legacy executable is missing its entry function"

assert (root / "NOTICE.txt").read_text().find("SIL OPEN FONT LICENSE") >= 0
assert not any((root / path).exists() for path in ["game.toml", "game.lock", "content", "tests", "assets/source"])
print(json.dumps({"release": release_id, "objects": len(release["objects"]), "bytes": total, "status": "PASS"}, indent=2))
