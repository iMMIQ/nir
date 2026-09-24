#!/usr/bin/env python3
"""Verify a format-1 NIR release from a directory or a static HTTP origin."""
import argparse
import gzip
import hashlib
import json
from pathlib import Path
import re
import sys
from urllib.error import HTTPError
from urllib.parse import urljoin, urlparse
from urllib.request import Request, urlopen

HASH = re.compile(r"[0-9a-f]{64}\Z")
MIME = {"html": "text/html", "js": "text/javascript", "json": "application/json",
        "wasm": "application/wasm", "png": "image/png", "wav": "audio/wav",
        "otf": "font/otf", "ttf": "font/ttf", "txt": "text/plain"}


def strict_json(data):
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError(f"duplicate JSON key: {key}")
            result[key] = value
        return result
    return json.loads(data, object_pairs_hook=pairs)


def valid_hash(value):
    assert isinstance(value, str) and HASH.fullmatch(value), f"invalid digest: {value}"


class Reader:
    def __init__(self, location):
        self.remote = location.startswith(("https://", "http://"))
        if self.remote:
            self.base = location.rstrip("/") + "/"
            assert urlparse(self.base).scheme in ("http", "https")
        else:
            self.base = Path(location).resolve()

    def get(self, path, mime=None, immutable=None):
        assert path and not path.startswith("/") and "\\" not in path and ":" not in path
        assert all(part not in ("", ".", "..") for part in path.split("/")), f"unsafe path: {path}"
        if self.remote:
            url = urljoin(self.base, path)
            assert url.startswith(self.base), f"escaped origin: {url}"
            with urlopen(Request(url, headers={"Accept-Encoding": "identity", "Cache-Control": "no-cache"}), timeout=30) as response:
                assert response.status == 200, f"HTTP {response.status}: {path}"
                assert response.url == url, f"redirected: {path}"
                if mime:
                    actual = response.headers.get_content_type()
                    assert actual == mime, f"MIME {path}: {actual} != {mime}"
                if immutable is not None:
                    cache = response.headers.get("Cache-Control", "").lower()
                    if immutable:
                        assert "immutable" in cache and "max-age" in cache, f"immutable cache missing: {path}"
                    else:
                        assert "no-cache" in cache or "no-store" in cache, f"mutable cache missing: {path}"
                return response.read()
        target = (self.base / path).resolve()
        assert target.is_relative_to(self.base), f"escaped root: {path}"
        return target.read_bytes()

    def missing(self, path):
        if not self.remote:
            return
        try:
            self.get(path)
        except HTTPError as error:
            assert error.code == 404, f"missing path returned HTTP {error.code}: {path}"
        else:
            raise AssertionError(f"missing path returned 200: {path}")


def verify(location, requested=None):
    reader = Reader(location)
    if requested is None:
        channel = strict_json(reader.get("channels/stable.json", "application/json", False))
        assert set(channel) == {"format", "release"} and channel["format"] == 1
        release_id = channel["release"]
    else:
        release_id = requested
    valid_hash(release_id)
    raw = reader.get(f"releases/{release_id}.json", "application/json", True)
    assert hashlib.sha256(raw).hexdigest() == release_id, "release digest mismatch"
    release = strict_json(raw)
    assert release["format"] == 1 and release["profile"] in ("dev", "release")
    assert set(release["launch"]) == {"html", "bootstrap"}
    objects = release["objects"]
    total = compressed_objects = compressed_bytes = 0
    for identity, obj in objects.items():
        valid_hash(identity)
        path = obj["path"]
        assert path.startswith(f"objects/{identity}."), f"invalid object path: {path}"
        ext = path.rsplit(".", 1)[-1]
        assert ext in MIME and obj["media_type"].split(";", 1)[0] == MIME[ext], f"object MIME: {path}"
        data = reader.get(path, MIME[ext], True)
        assert len(data) == obj["bytes"], f"size mismatch: {identity}"
        assert hashlib.sha256(data).hexdigest() == identity, f"digest mismatch: {identity}"
        if not reader.remote:
            sidecar = reader.base / (path + ".gz")
            if sidecar.exists():
                encoded = sidecar.read_bytes()
                assert gzip.decompress(encoded) == data, f"gzip mismatch: {identity}"
                compressed_objects += 1
                compressed_bytes += len(encoded)
        total += len(data)
    for identity in [release["program"], *release["engine"].values(),
                     *release["launch"].values(), *release["notices"]]:
        assert identity in objects, f"missing release root: {identity}"
    for name, identity, mime in [("index.html", release["launch"]["html"], "text/html"),
                                 ("bootstrap.js", release["launch"]["bootstrap"], "text/javascript")]:
        data = reader.get(f"releases/{release_id}/{name}", mime, True)
        assert hashlib.sha256(data).hexdigest() == identity, f"fixed launch mismatch: {name}"
        assert data == reader.get(objects[identity]["path"], mime, True)
    wasm = reader.get(objects[release["engine"]["wasm"]]["path"], "application/wasm", True)
    assert wasm.startswith(b"\0asm\x01\0\0\0"), "not actual WASM"
    if requested is None:
        for name, identity, mime in [("index.html", release["launch"]["html"], "text/html"),
                                     ("bootstrap.js", release["launch"]["bootstrap"], "text/javascript")]:
            assert hashlib.sha256(reader.get(name, mime, False)).hexdigest() == identity, f"root launch mismatch: {name}"
    reader.missing("releases/" + "0" * 64 + ".json" if release_id != "0" * 64 else "releases/" + "f" * 64 + ".json")
    reader.missing("objects/" + "0" * 64 + ".json")
    if not reader.remote:
        if requested is None:
            assert hashlib.sha256(reader.get("NOTICE.txt")).hexdigest() in release["notices"], "root notice mismatch"
        assert not any((reader.base / path).exists() for path in ["game.toml", "game.lock", "content", "tests", "assets/source"])
        if "verify_runtime" not in globals():
            from verify_runtime import verify_runtime
        verify_runtime(reader.base, release)
    return {"release": release_id, "profile": release["profile"], "objects": len(objects),
            "bytes": total, "gzip_objects": compressed_objects, "gzip_bytes": compressed_bytes, "status": "PASS"}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("location", help="release directory or root URL")
    parser.add_argument("--release", help="verify a staged digest without reading a channel")
    args = parser.parse_args()
    print(json.dumps(verify(args.location, args.release), indent=2))
