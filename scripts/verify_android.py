"""Verify the structure and signature of a packaged Android game.

No device is required: this checks that the APK is a valid zip, carries the
unchanged native data graph, embeds a plausible NativeActivity manifest, and
bears an APK Signature Scheme v2 block. When `apksigner`/`aapt2` from the
Android build tools are available (env APKSIGNER/AAPT2 or PATH), their
gold-standard verdicts are required as well.
"""
import hashlib
import json
import os
import shutil
import struct
import subprocess
import sys
import zipfile
from pathlib import Path

root = Path(sys.argv[1]).resolve()
apk = root / "Game.apk"
assert apk.is_file(), "Game.apk missing"
assert (root / "README.txt").is_file(), "README.txt missing"
assert (root / "NOTICE.txt").is_file(), "NOTICE.txt missing"

with zipfile.ZipFile(apk) as bundle:
    names = bundle.namelist()
    assert len(names) == len(set(names)), "duplicate APK entries"
    manifest = bundle.read("AndroidManifest.xml")
    player = bundle.read("lib/arm64-v8a/libplayer.so")
    release = bundle.read("assets/data/release.txt").decode().strip()
    raw = bundle.read(f"assets/data/releases/{release}.json")
    objects = [n for n in names if n.startswith("assets/data/objects/")]
    assert objects, "content objects missing from APK"
    assert not any(n.endswith((".wasm", ".js")) for n in names), "web engine files leaked into APK"
    assert not any(n.startswith("META-INF/") for n in names), "v1 JAR signing artifacts present"
    # The shared library must be stored uncompressed so the loader can map it
    # directly; Android 6+ rejects compressed .so entries with the modern
    # extraction rules.
    lib_info = bundle.getinfo("lib/arm64-v8a/libplayer.so")
    assert lib_info.compress_type == zipfile.ZIP_STORED, "libplayer.so must be STORED"

assert hashlib.sha256(raw).hexdigest() == release, "release manifest digest mismatch"
native = json.loads(raw)
assert hashlib.sha256(player).hexdigest() == native["player"], "player digest mismatch"
assert player.startswith(b"\x7fELF"), "libplayer.so is not an ELF image"

# Binary AXML: a resource chunk whose header is RES_XML_TYPE, and the string
# pool must carry the identity strings the launcher resolves.
assert manifest[:4] == struct.pack("<HH", 3, 8), "AndroidManifest.xml is not binary AXML"
text = manifest.decode("utf-16-le", errors="ignore")
for needle in ["one.nir.", "android.app.NativeActivity", "player", "android.intent.action.MAIN"]:
    assert needle in text, f"manifest string pool lacks {needle!r}"

# APK Signature Scheme v2: the block sits between the last entry and the
# central directory, closed by its 16-byte magic.
data = apk.read_bytes()
eocd = -1
i = len(data) - 22
while i >= 0 and i >= len(data) - 65557:
    if data[i : i + 4] == b"PK\x05\x06":
        (comment,) = struct.unpack_from("<H", data, i + 20)
        if i + 22 + comment == len(data):
            eocd = i
            break
    i -= 1
assert eocd > 0, "zip end-of-central-directory missing"
(cd_offset,) = struct.unpack_from("<I", data, eocd + 16)
assert data[cd_offset - 16 : cd_offset] == b"APK Sig Block 42", "v2 signing block magic missing"
(size,) = struct.unpack_from("<Q", data, cd_offset - 24)
assert 32 <= size <= cd_offset, "v2 signing block size out of range"
print("PASS structure: %d entries, release %s, v2 signing block present" % (len(names), release[:12]))

# Gold-standard verdicts whenever the Android build tools are reachable.
def tool(name):
    explicit = os.environ.get(name.upper())
    if explicit and Path(explicit).exists():
        return explicit
    return shutil.which(name.lower())

apksigner = tool("APKSIGNER")
if apksigner:
    try:
        run_out = subprocess.run(
            [apksigner, "verify", "--verbose", str(apk)],
            check=True,
            capture_output=True,
            text=True,
            timeout=120,
        )
    except FileNotFoundError:
        # apksigner is a shell script; its #!/bin/bash shebang is absent on
        # some hosts (NixOS), where bash lives elsewhere.
        run_out = subprocess.run(
            ["bash", apksigner, "verify", "--verbose", str(apk)],
            check=True,
            capture_output=True,
            text=True,
            timeout=120,
        )
    out = run_out.stdout
    assert "Verified using v2 scheme (APK Signature Scheme v2): true" in out, out
    assert "DOES NOT VERIFY" not in out, out
    print("PASS apksigner: v2 signature verified")
else:
    print("SKIP apksigner (not found; structural checks only)")

aapt2 = tool("AAPT2")
if aapt2:
    badging = subprocess.run(
        [aapt2, "dump", "badging", str(apk)],
        check=True,
        capture_output=True,
        text=True,
        timeout=120,
    ).stdout
    assert "package: name='one.nir." in badging, badging
    assert "sdkVersion:'26'" in badging, badging
    assert "targetSdkVersion:'29'" in badging, badging
    assert "native-code: 'arm64-v8a'" in badging, badging
    assert "launchable-activity: name='android.app.NativeActivity'" in badging, badging
    print("PASS aapt2: badging verified")
else:
    print("SKIP aapt2 (not found; structural checks only)")

reports = Path("reports/android")
reports.mkdir(parents=True, exist_ok=True)
(reports / "verify.json").write_text(
    json.dumps(
        {
            "apk": str(apk),
            "entries": len(names),
            "release": release,
            "player_sha256": native["player"],
            "objects": len(objects),
            "apksigner": bool(apksigner),
            "aapt2": bool(aapt2),
        },
        indent=2,
    )
    + "\n"
)
