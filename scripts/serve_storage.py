#!/usr/bin/env python3
"""Build the neutral long-voice fixture for isolated browser storage checks."""
import hashlib
import json
import os
import shutil
import subprocess
import wave
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

root = Path(__file__).resolve().parent.parent
project = root / "reports/storage-fixture"
sdk = Path(os.environ.get("NIR_STORAGE_SDK", str(root / "dist/sdk"))).resolve()
shutil.copytree(root / "examples/rain-letters", project,
                ignore=shutil.ignore_patterns("dist", ".nir", "reports"), dirs_exist_ok=True)
fragment = project / "content/ch01/story.nir.json"
content = json.loads(fragment.read_text())
# Save transport checks need an authored signed zero in the real scene.
content["scenes"]["station"][0]["x"] = -0.0
line = content["cues"]["intro"]["effects"][0]["effect"]
line.update(reveal_us="0", speaker="speaker.aki")
for name in ["spoken", "unrelated-voice"]:
    content["cues"]["intro"]["effects"].append({"id": name, "scope": "session", "effect": {
        "type": "audio", "asset": "audio.bgm", "bus": "voice", "looped": False}})
content["functions"]["main"]["blocks"]["wait_intro"]["ops"].append({"id": "reading-boundary", "operation": {
    "type": "dialogue_voice", "task": "line", "voice": "spoken", "wait": "sampled_remaining"}})
fragment.write_text(json.dumps(content, ensure_ascii=False, indent=2) + "\n")
config = project / "config/player.toml"
config.write_text(config.read_text().replace('[defaults]', '[defaults]\nauto_delay_policy = "fixed"')
                  .replace('auto_delay_us = "1200000"', 'auto_delay_us = "100000"'))
audio = project / "assets/source/bgm.wav"
with wave.open(str(audio), "rb") as source:
    params, frames = source.getparams(), source.readframes(source.getnframes())
with wave.open(str(audio), "wb") as target:
    target.setparams(params)
    target.writeframes(frames * 3)
for command in [[str(root / "dist/novelc"), "-p", str(project), "resolve", "--sdk", str(sdk)],
                [str(root / "dist/novelc"), "-p", str(project), "build", "--locked", "--sdk", str(sdk)]]:
    subprocess.run(command, check=True)
web = project / "dist/full/web"
release_id = json.loads((web / "channels/stable.json").read_text())["release"]
release = json.loads((web / f"releases/{release_id}.json").read_text())
engine_files = {"host": "host.js", "js": "player_web.js", "wasm": "player_web_bg.wasm",
                "runtime_worker": "runtime-worker.js", "asset_worker": "asset-worker.js"}
engine_sha = {}
for key, name in engine_files.items():
    data = (web / release["objects"][release["engine"][key]]["path"]).read_bytes()
    assert data == (sdk / name).read_bytes()
    engine_sha[key] = hashlib.sha256(data).hexdigest()
assert not list((web / "objects").glob("*.wav"))
assert all(obj["media_type"] == "audio/mpeg" for obj in release["objects"].values()
           if obj["media_type"].startswith("audio/"))
(root / "reports/storage-fixture-build.json").write_text(json.dumps({
    "release": release_id, "engineSha256": engine_sha,
    "scope": "Neutral character-voice fixture, exact selected SDK, MP3-only runtime; source WAV for compiler conversion only."}, indent=2) + "\n")
ThreadingHTTPServer(("127.0.0.1", 4259), partial(SimpleHTTPRequestHandler, directory=str(web))).serve_forever()
