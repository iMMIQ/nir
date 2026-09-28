"""Verify and exercise a distributed native Linux game with isolated save data."""
import hashlib
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

root = Path(sys.argv[1]).resolve()
assert sys.platform.startswith("linux"), "Run native acceptance on Linux"
exe = root / "Game"
assert os.access(exe, os.X_OK), "Game must keep its executable bit"
release = (root / "data/release.txt").read_text().strip()
raw = (root / f"data/releases/{release}.json").read_bytes()
assert hashlib.sha256(raw).hexdigest() == release
manifest = json.loads(raw)
assert hashlib.sha256(exe.read_bytes()).hexdigest() == manifest["player"]
assert not list(root.rglob("*.wasm")) and not list(root.rglob("*.js"))
subprocess.run([str(exe), "--verify"], check=True, timeout=90)
reports = Path("reports/linux").resolve()
reports.mkdir(parents=True, exist_ok=True)
Path("target/tmp").mkdir(parents=True, exist_ok=True)
with tempfile.TemporaryDirectory(dir="target/tmp", prefix="native-saves-") as temp:
    for attempt in range(2):
        report = reports / f"smoke-{attempt}.json"
        report.unlink(missing_ok=True)
        subprocess.run([str(exe), "--hidden", "--data-dir", str(Path(temp).resolve()), "--smoke-report", str(report)], check=True, timeout=100)
        result = json.loads(report.read_bytes())
        assert result["ok"], result
        assert result["state"]["backend"] == "vulkan", result["state"]["backend"]
        assert result["state"]["frames"] > 0
        assert result["save_revision"] == attempt + 1, result
        print(f"PASS native run {attempt + 1}: {result['elapsed_ms']} ms, {result['audio_starts']} audio starts")
