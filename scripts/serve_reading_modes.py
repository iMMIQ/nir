#!/usr/bin/env python3
"""Build voiced neutral branches with the current SDK; serve a reading matrix."""
import copy
import hashlib
import json
import math
import shutil
import struct
import subprocess
import threading
import time
import wave
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

root = Path(__file__).resolve().parent.parent
sdk = root / "dist/sdk"
cli = root / "dist/novelc"
sha = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()


def build(policy):
    wait_policy = "parallel" if policy in ("parallel_interaction", "author_gate") else policy
    voice_scope = "interaction" if policy == "parallel_interaction" else "session"
    project = root / "reports/reading-modes" / policy
    shutil.copytree(root / "examples/rain-letters", project,
                    ignore=shutil.ignore_patterns("dist", ".nir", "reports"), dirs_exist_ok=True)
    config = project / "config/player.toml"
    config.write_text(config.read_text().replace('[defaults]', '[defaults]\nauto_delay_policy = "fixed"')
                      .replace('auto_delay_us = "1200000"', 'auto_delay_us = "600000"')
                      .replace('prefetch_media = true', 'prefetch_media = false'))
    catalog = project / "assets/catalog.toml"
    for index, seconds in enumerate([4, 2, 3], 1):
        audio = project / f"assets/source/reading-voice-{index}.wav"
        rate = 22050
        with wave.open(str(audio), "wb") as output:
            output.setparams((1, 2, rate, 0, "NONE", "not compressed"))
            output.writeframes(b"".join(struct.pack("<h", int(1600 * math.sin(2 * math.pi * (220 + 20 * index) * n / rate)
                                                       * min(1, n / 220, (seconds * rate - n) / 220)))
                                       for n in range(seconds * rate)))
        with catalog.open("a") as output:
            output.write(f'\n[[assets]]\nid = "reading.voice.{index}"\nkind = "audio"\nsource = "source/reading-voice-{index}.wav"\nrights = "CC0-1.0"\n')
    fragment = project / "content/ch01/story.nir.json"
    content = json.loads(fragment.read_text())
    # The shortened reading sequence bypasses the original entrance. Both
    # original branches still need its actor, including the stay branch clip.
    content["cues"]["opening"]["effects"][0]["effect"]["scene"] = "together"
    blocks = content["functions"]["main"]["blocks"]
    for index, (name, wait, next_block) in enumerate([
        ("intro", "wait_intro", "arrival"),
        ("arrival", "wait_arrival", "after_gate"),
        ("after_gate", "wait_after", "choose"),
    ], 1):
        line = copy.deepcopy(content["cues"][name]["effects"][0])
        line["effect"]["reveal_us"] = "160000" if index == 1 else "0"
        content["cues"][name]["effects"] = [line, {
            "id": "spoken", "scope": voice_scope, "effect": {
                "type": "audio", "asset": f"reading.voice.{index}", "bus": "voice", "looped": False,
            },
        }]
        blocks[wait]["ops"] = [{"id": f"bind.{name}", "operation": {
            "type": "dialogue_voice", "task": "line", "voice": "spoken", "wait": wait_policy,
        }}]
        blocks[wait]["terminator"]["next"] = next_block
    if policy == "author_gate":
        # Keep the original mid-line Gate, explicit Sfx await and continuation.
        # Reading preferences may speed up text, but cannot release this Gate.
        blocks["start"]["terminator"]["next"] = "letter"
        content["cues"]["letter"]["effects"][0]["effect"]["reveal_us"] = "160000"
        content["cues"]["letter"]["effects"].append({
            "id": "spoken", "scope": "session", "effect": {
                "type": "audio", "asset": "reading.voice.1", "bus": "voice", "looped": False,
            },
        })
        blocks["gate"]["ops"] = [{"id": "bind.letter", "operation": {
            "type": "dialogue_voice", "task": "line", "voice": "spoken", "wait": "parallel",
        }}]
        content["cues"]["bell"]["effects"][0]["effect"]["asset"] = "reading.voice.2"
    fragment.write_text(json.dumps(content, ensure_ascii=False, indent=2) + "\n")
    for command in [[str(cli), "-p", str(project), "resolve", "--sdk", str(sdk)],
                    [str(cli), "-p", str(project), "build", "--locked", "--sdk", str(sdk)]]:
        subprocess.run(command, check=True)
    web = project / "dist/full/web"
    release_id = json.loads((web / "channels/stable.json").read_text())["release"]
    release = json.loads((web / f"releases/{release_id}.json").read_text())
    engine_files = {"host": "host.js", "js": "player_web.js", "wasm": "player_web_bg.wasm",
                    "runtime_worker": "runtime-worker.js", "asset_worker": "asset-worker.js"}
    engine_sha = {}
    for key, name in engine_files.items():
        engine_sha[key] = sha(web / release["objects"][release["engine"][key]]["path"])
        assert engine_sha[key] == sha(sdk / name)
    assert not list((web / "objects").glob("*.wav"))
    program = json.loads((web / release["objects"][release["program"]]["path"]).read_text())["program"]
    assets = {}
    for digest in program["catalogs"].values():
        assets.update(json.loads((web / release["objects"][digest]["path"]).read_text())["assets"])
    assert all(release["objects"][a["object"]]["media_type"] == "audio/mpeg" for a in assets.values() if a["kind"] == "audio")
    return web, {"fixture": policy, "policy": wait_policy, "voiceScope": voice_scope,
                 "release": release_id, "engine": release["engine"],
                 "engineSha256": engine_sha,
                 "voiceObjects": [release["objects"][assets[f"reading.voice.{i}"]["object"]]["path"] for i in range(1, 4)]}


class Handler(SimpleHTTPRequestHandler):
    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/__reading_fixture/delay":
            query = parse_qs(url.query)
            if "path" in query:
                path = query["path"][0]
                assert path in self.server.fixture["voiceObjects"]
                ms = int(query.get("ms", ["0"])[0])
                assert 0 <= ms <= 5000
                self.server.delays[path] = ms
            data = json.dumps(self.server.fixture).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        delay = self.server.delays.get(url.path.lstrip("/"), 0)
        if delay:
            time.sleep(delay / 1000)
        super().do_GET()


if __name__ == "__main__":
    rows = []
    servers = []
    for port, policy in enumerate(["parallel", "after_voice", "sampled_remaining", "parallel_interaction", "author_gate"], 4271):
        web, fixture = build(policy)
        fixture["port"] = port
        server = ThreadingHTTPServer(("127.0.0.1", port), partial(Handler, directory=str(web)))
        server.fixture = fixture
        server.delays = {}
        rows.append(fixture)
        servers.append(server)
    (root / "reports/reading-modes/build.json").write_text(json.dumps({"fixtures": rows, "scope": "Neutral voiced choice branches, current SDK, MP3-only runtime; source WAV generated only for compiler conversion."}, indent=2) + "\n")
    for server in servers[1:]:
        threading.Thread(target=server.serve_forever, daemon=True).start()
    servers[0].serve_forever()
