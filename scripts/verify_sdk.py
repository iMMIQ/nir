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
    config = json.loads(run("-p", "story", "config"))
    assert config["theme.slots.dialogue.main"]["value"] == "builtin.dialogue"
    theme = root / "story/themes/rain/theme.toml"
    original_theme = theme.read_text()
    theme.write_text(original_theme.replace('builtin.dialogue"', 'builtin.dialogue.top"'))
    run("-p", "story", "check", "--locked")
    run("-p", "story", "build", "--locked")
    assert json.loads(channel.read_text())["release"] != first
    theme.write_text(original_theme)
    fragment = root / "story/content/ch01/story.nir.json"
    original = fragment.read_bytes()
    content = json.loads(original)
    first_scene = next(iter(content["scenes"].values()))
    first_scene[0]["asset"] = "missing.test.resource"
    fragment.write_text(json.dumps(content, indent=2))
    report = json.loads(run("-p", "story", "--diagnostics", "json", "check", success=False))
    assert report["format"] == 1 and report["diagnostic"]["code"] == "E_ASSET_TYPE"
    detail = report["diagnostic"]["details"]
    assert detail["source"]["file"] == "content/ch01/story.nir.json"
    assert detail["source"]["pointer"].endswith("/0/asset") and detail["source"]["line"] > 1
    assert "missing.test.resource" in detail["references"] and detail["hint"]
    fragment.write_bytes(original)
    with (kit / "sdk/host.js").open("a") as f:
        f.write("\n// intentional SDK drift\n")
    error = run("-p", "story", "check", "--locked", success=False)
    assert "E_LOCK_DRIFT" in error, error
    print(json.dumps({"status": "PASS", "commands": ["init", "resolve", "doctor", "check --locked", "test", "build --locked"], "repeat_build_same_release": True, "sdk_drift_rejected": True, "cargo_on_path": False, "structured_source_diagnostic": True, "author_theme_edit_without_engine_rebuild": True, "resolved_configuration": True}, indent=2))
