#!/usr/bin/env python3
"""Exercise the distributed CLI/SDK without Cargo or source files in its directory."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

source = Path("dist").resolve()
Path("target/tmp").mkdir(parents=True, exist_ok=True)
with tempfile.TemporaryDirectory(dir="target/tmp", prefix="standalone-") as temp:
    root = Path(temp).resolve()
    kit = root / "kit"
    kit.mkdir()
    shutil.copy2(source / "novelc", kit / "novelc")
    shutil.copytree(source / "sdk", kit / "sdk")
    env = {**os.environ, "PATH": "/usr/bin:/bin"}
    env.pop("NIR_SDK", None)
    def run(*args, success=True):
        p = subprocess.run([str(kit / "novelc"), *args], cwd=root, env=env, text=True, capture_output=True)
        assert (p.returncode == 0) == success, p.stdout + p.stderr
        return p.stdout + p.stderr
    run("init", "story")
    for cmd in [("resolve",), ("doctor",), ("check", "--locked"), ("test",), ("build", "--locked")]:
        run("-p", "story", *cmd)
    channel = root / "story/dist/full/web/channels/stable.json"
    first = json.loads(channel.read_text())["release"]
    run("-p", "story", "build", "--locked")
    assert json.loads(channel.read_text())["release"] == first
    with (kit / "sdk/host.js").open("a") as f:
        f.write("\n// intentional SDK drift\n")
    error = run("-p", "story", "check", "--locked", success=False)
    assert "E_LOCK_DRIFT" in error, error
    print(json.dumps({"status": "PASS", "commands": ["init", "resolve", "doctor", "check --locked", "test", "build --locked"], "repeat_build_same_release": True, "sdk_drift_rejected": True, "cargo_on_path": False}, indent=2))
