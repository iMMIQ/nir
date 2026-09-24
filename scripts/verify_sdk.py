#!/usr/bin/env python3
"""Exercise the distributed CLI/SDK without Cargo or source files in its directory."""
import json
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from http.server import ThreadingHTTPServer, SimpleHTTPRequestHandler
from threading import Thread
from verify_runtime import verify_runtime

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
    def verify_split_release(directory):
        channel = json.loads((directory / "channels/stable.json").read_bytes())
        release_id = channel["release"]
        raw = (directory / "releases" / f"{release_id}.json").read_bytes()
        assert hashlib.sha256(raw).hexdigest() == release_id
        release = json.loads(raw)
        verify_runtime(directory, release)
    run("init", "story", "--template", "web-basic")
    for cmd in [("resolve",), ("doctor",), ("check", "--locked"), ("test",), ("build", "--locked")]:
        run("-p", "story", *cmd)
    verify_split_release(root / "story/dist/full/web")
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
    # The default is an independent author project with its own master font.
    env["PATH"] = ""  # even Python and system font tools are unavailable to the CLI
    run("init", "minimal")
    for cmd in [("resolve",), ("check", "--locked"), ("test",), ("build", "--locked")]:
        run("-p", "minimal", *cmd)
    verify_split_release(root / "minimal/dist/full/web")
    # A release can be imported into a static directory without changing its
    # channel; only an explicit compare-and-swap promotion selects it.
    source_release = root / "story/dist/full/web"
    second = json.loads((source_release / "channels/stable.json").read_text())["release"]
    published = root / "published"
    for identity in (first, second):
        run("release", "stage", "--source", str(source_release), "--directory", str(published), "--release", identity)
    assert not (published / "channels/stable.json").exists()
    run("release", "promote", "--directory", str(published), "--release", first, "--expect", "none")
    assert json.loads(run("release", "verify", "--directory", str(published)))["release"] == first
    assert "E_CHANNEL_CAS" in run("release", "promote", "--directory", str(published), "--release", second, "--expect", "none", success=False)
    first_objects = set(json.loads((published / "releases" / f"{first}.json").read_text())["objects"])
    second_manifest = json.loads((published / "releases" / f"{second}.json").read_text())
    changed = next(identity for identity in second_manifest["objects"] if identity not in first_objects)
    changed_path = published / second_manifest["objects"][changed]["path"]
    original_object = changed_path.read_bytes()
    changed_path.write_bytes(bytes([original_object[0] ^ 1]) + original_object[1:])
    assert "E_DIGEST" in run("release", "promote", "--directory", str(published), "--release", second, "--expect", first, success=False)
    assert json.loads((published / "channels/stable.json").read_text())["release"] == first
    assert "E_DIGEST" in run("release", "stage", "--source", str(source_release), "--directory", str(published), "--release", second, success=False)
    changed_path.write_bytes(original_object)
    changed_path.unlink()
    assert "No such file" in run("release", "promote", "--directory", str(published), "--release", second, "--expect", first, success=False)
    run("release", "stage", "--source", str(source_release), "--directory", str(published), "--release", second)

    # Probe server failures using GET. Correct files carry immutable cache on
    # releases/objects and no-cache on mutable entry and channel files.
    class ReleaseHandler(SimpleHTTPRequestHandler):
        fault = None
        def __init__(self, *args, **kwargs):
            super().__init__(*args, directory=str(published), **kwargs)
        def log_message(self, *args):
            pass
        def do_GET(self):
            if self.fault == "missing" and self.path.startswith("/releases/" + "0" * 64):
                self.send_response(200); self.end_headers(); self.wfile.write(b"fallback"); return
            return super().do_GET()
        def guess_type(self, path):
            ext = Path(path).suffix
            mime = {".html":"text/html", ".js":"text/javascript", ".json":"application/json", ".wasm":"application/wasm", ".png":"image/png", ".wav":"audio/wav", ".otf":"font/otf", ".ttf":"font/ttf", ".txt":"text/plain"}.get(ext, "application/octet-stream")
            if self.fault == "mime" and path.endswith(f"{first}.json"):
                return "text/plain"
            return mime
        def end_headers(self):
            immutable = self.path.startswith(("/releases/", "/objects/"))
            cache = "public, max-age=31536000, immutable" if immutable else "no-cache"
            if self.fault == "cache" and self.path.endswith(f"{first}.json"):
                cache = "no-cache"
            self.send_header("Cache-Control", cache)
            super().end_headers()
    server = ThreadingHTTPServer(("127.0.0.1", 0), ReleaseHandler)
    worker = Thread(target=server.serve_forever, daemon=True)
    worker.start()
    url = f"http://127.0.0.1:{server.server_port}"
    try:
        for fault, code in (("mime", "E_VERIFY_MIME"), ("cache", "E_VERIFY_CACHE"), ("missing", "E_VERIFY_MISSING")):
            ReleaseHandler.fault = fault
            assert code in run("release", "verify", "--url", url, success=False)
    finally:
        server.shutdown(); worker.join()
    run("release", "promote", "--directory", str(published), "--release", second, "--expect", first)
    run("release", "rollback", "--directory", str(published), "--to", first, "--expect", second)
    assert json.loads((published / "channels/stable.json").read_text())["release"] == first

    dev_out = root / "dev-output"
    run("-p", "minimal", "build", "--profile", "dev", "--locked", "--out", str(dev_out))
    dev_release = json.loads((dev_out / "channels/stable.json").read_text())["release"]
    assert "E_RELEASE_PROFILE" in run("release", "promote", "--directory", str(dev_out), "--release", dev_release, "--expect", dev_release, success=False)
    assert "E_RELEASE_PROFILE" in run("release", "stage", "--source", str(dev_out), "--directory", str(root / "rejected-dev"), success=False)
    report = json.loads((root / "minimal/reports/build.json").read_text())
    font = report["fonts"]["font.reader"]
    assert font["output_bytes"] < font["source_bytes"] / 20
    assert font["cache_hit"]
    first_font = font["object"]
    text = root / "minimal/content/main/texts/zh-Hans.json"
    text.write_text(text.read_text().replace("春天", "鲸鱼"))
    status=json.loads(run("-p", "minimal", "text", "status", "--json"))
    assert not status["ready"] and any(i["code"]=="E_TEXT_SOURCE_CHANGED" for i in status["issues"])
    assert "E_TEXT_SOURCE_CHANGED" in run("-p", "minimal", "build", "--locked", success=False)
    run("-p", "minimal", "text", "update", "--id", "intro", "--meaning", "preserve")
    assert "E_TRANSLATION_STALE" in run("-p", "minimal", "build", "--locked", success=False)
    run("-p", "minimal", "text", "review", "--id", "intro", "--locale", "en")
    run("-p", "minimal", "build", "--locked")
    report = json.loads((root / "minimal/reports/build.json").read_text())
    assert report["fonts"]["font.reader"]["object"] != first_font
    assert not report["fonts"]["font.reader"]["cache_hit"]
    changed = report["release"]
    shutil.rmtree(root / "minimal/.nir")
    run("-p", "minimal", "build", "--locked")
    assert json.loads((root / "minimal/reports/build.json").read_text())["release"] == changed
    with (kit / "sdk/host.js").open("a") as f:
        f.write("\n// intentional SDK drift\n")
    error = run("-p", "story", "check", "--locked", success=False)
    assert "E_LOCK_DRIFT" in error, error
    print(json.dumps({"status": "PASS", "commands": ["init", "resolve", "doctor", "check --locked", "test", "build --locked"], "repeat_build_same_release": True, "sdk_drift_rejected": True, "cargo_on_path": False, "structured_source_diagnostic": True, "author_theme_edit_without_engine_rebuild": True, "resolved_configuration": True, "minimal_template": True, "new_chinese_without_external_font_tools": True, "minimal_cli_path_empty": True, "clean_cache_reproducible": True, "source_change_and_translation_review": True, "release_stage_promote_rollback": True, "release_verifier_without_external_runtime": True, "release_corruption_and_cas_rejected": True, "remote_mime_cache_and_404_checked": True, "dev_promotion_rejected": True}, indent=2))
