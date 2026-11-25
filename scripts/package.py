#!/usr/bin/env python3
"""Package the tested, standalone NIR v2 SDK, CLI, and Rain Letters Web demo."""
import gzip
import hashlib
import io
from pathlib import Path
import shutil
import subprocess
import tarfile
import tomllib

version = tomllib.loads(Path("Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
dist = Path("dist")
bundle = dist / "bundle"
actual = subprocess.check_output([str(dist / "novelc"), "--version"], text=True).strip()
assert actual == f"novelc {version}", f"CLI version mismatch: {actual}"

if bundle.exists():
    shutil.rmtree(bundle)
bundle.mkdir(exist_ok=True)
shutil.copy2(dist / "novelc", bundle / "novelc")
shutil.copytree(dist / "sdk", bundle / "sdk", dirs_exist_ok=True)
shutil.copytree("docs", bundle / "docs", dirs_exist_ok=True, ignore=shutil.ignore_patterns("specs", "validation"))
for name in ("LICENSE", "COPYING", "LICENSE-NOTICE.md"):
    shutil.copy2(name, bundle / name)

source = dist / "rain-letters-web"
required_web = [
    "index.html",
    "style.css",
    "app.js",
    "player_web.js",
    "player_web_bg.wasm",
    "game.nirpak",
]
optional_web = [
    "manifest.webmanifest",
    "sw.js",
    "fallback-cjk.otf",
    "fallback-cjk.ttf",
]
web = bundle / "rain-letters-web"
web.mkdir(exist_ok=True)
for name in required_web:
    src = source / name
    assert src.is_file(), f"missing required web artifact: {src}"
    shutil.copy2(src, web / name)
for name in optional_web:
    src = source / name
    if src.is_file():
        shutil.copy2(src, web / name)

(bundle / "README.md").write_text(
    f"""# NIR {version} 叙事引擎 SDK 与 CLI · Linux x86_64

本包提供创建、检查、预览和构建 NIR v2 叙事作品所需的引擎运行时与作者工具。

- `novelc`：作品开发 CLI（init / check / build / convert / dev / serve / i18n）。
- `sdk/`：配套 Web（WebGPU / WebGL2 WASM）、Linux 原生播放器与 Android 原生库。
- `docs/`：架构说明文档。
- `rain-letters-web/`：《雨后书简》示例工程的 Web 构建。

解压后，在本目录创建作品，无需安装 Rust：

```sh
./novelc init my-story --title "My Story"
./novelc check my-story
./novelc dev my-story
./novelc build my-story --target web
./novelc build my-story --target native
./novelc build my-story --target android
```

运行附带的示例工程：

```sh
./novelc serve rain-letters-web
```

在桌面或移动浏览器打开 http://127.0.0.1:4173/ 。

许可：LGPL-3.0-or-later；完整许可见 LICENSE、COPYING 和 LICENSE-NOTICE.md。对应源码随同版本源码包提供：https://github.com/iMMIQ/nir/releases 。
""",
    encoding="utf-8",
)


def canonical(info):
    info.uid = info.gid = 0
    info.uname = info.gname = ""
    info.mtime = 0
    return info


def archive(path, root, paths, prefix):
    with (
        path.open("wb") as file,
        gzip.GzipFile(filename="", mode="wb", fileobj=file, mtime=0) as gz,
        tarfile.open(fileobj=gz, mode="w") as tar,
    ):
        for p in sorted(paths):
            tar.add(p, arcname=str(Path(prefix) / p.relative_to(root)), recursive=False, filter=canonical)


archive(
    dist / f"nir-{version}-linux-x86_64.tar.gz",
    bundle,
    [p for p in bundle.rglob("*") if p.is_file()],
    f"nir-{version}",
)

# Archive committed bytes, never runner-generated lock files or local changes.
source_tar = subprocess.check_output(["git", "archive", "--format=tar", f"--prefix=nir-{version}-source/", "HEAD"])
with (
    (dist / f"nir-{version}-source.tar.gz").open("wb") as file,
    gzip.GzipFile(filename="", mode="wb", fileobj=file, mtime=0) as gz,
    tarfile.open(fileobj=gz, mode="w") as target,
    tarfile.open(fileobj=io.BytesIO(source_tar), mode="r:") as original,
):
    for member in original:
        target.addfile(canonical(member), original.extractfile(member) if member.isfile() else None)

checks = []
for name in [f"nir-{version}-linux-x86_64.tar.gz", f"nir-{version}-source.tar.gz"]:
    path = dist / name
    checks.append(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {name}")
    print(f"{name}: {path.stat().st_size:,} bytes")
(dist / "SHA256SUMS").write_text("\n".join(checks) + "\n", encoding="utf-8")
