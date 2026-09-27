"""Verify and exercise a distributed native Windows game with isolated save data."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

root = Path(sys.argv[1]).resolve()
assert os.name == "nt", "Run native acceptance on Windows"
exe = root / "Game.exe"
release = (root / "data/release.txt").read_text().strip()
raw = (root / f"data/releases/{release}.json").read_bytes()
assert hashlib.sha256(raw).hexdigest() == release
manifest = json.loads(raw)
assert hashlib.sha256(exe.read_bytes()).hexdigest() == manifest["player"]
assert not list(root.rglob("*.wasm")) and not list(root.rglob("*.js"))
subprocess.run([str(exe), "--verify"], check=True, timeout=90)
reports = Path("reports/windows").resolve()
reports.mkdir(parents=True, exist_ok=True)
Path("target/tmp").mkdir(parents=True, exist_ok=True)
with tempfile.TemporaryDirectory(dir="target/tmp", prefix="native-saves-") as temp:
    for attempt in range(2):
        report = reports / f"smoke-{attempt}.json"
        report.unlink(missing_ok=True)
        subprocess.run([str(exe), "--hidden", "--data-dir", str(Path(temp).resolve()), "--smoke-report", str(report)], check=True, timeout=100)
        result = json.loads(report.read_bytes())
        assert result["ok"], result
        assert result["state"]["backend"] == "dx12"
        assert result["state"]["frames"] > 0
        assert result["save_revision"] == attempt + 1, result
        print(f"PASS native run {attempt + 1}: {result['elapsed_ms']} ms, {result['audio_starts']} audio starts")
