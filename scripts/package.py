#!/usr/bin/env python3
"""Package only the selected immutable release and the tested, standalone SDK."""
import gzip
import hashlib
import json
from pathlib import Path
import shutil
import tarfile
from audit_publication import source_files

dist=Path("dist");bundle=dist/"bundle"
if bundle.exists():shutil.rmtree(bundle)
bundle.mkdir(exist_ok=True)
shutil.copy2(dist/"novelc",bundle/"novelc")
shutil.copytree(dist/"sdk",bundle/"sdk",dirs_exist_ok=True)
shutil.copytree("docs",bundle/"docs",dirs_exist_ok=True,ignore=shutil.ignore_patterns("specs"))
for name in ("LICENSE", "COPYING", "LICENSE-NOTICE.md"):
    shutil.copy2(name,bundle/name)
source=dist/"rain-letters-web"
channel=json.loads((source/"channels/stable.json").read_text())
manifest_path=f"releases/{channel['release']}.json"
manifest=json.loads((source/manifest_path).read_text())
selected=["index.html","bootstrap.js","NOTICE.txt","channels/stable.json",manifest_path,*[o["path"] for o in manifest["objects"].values()]]
web=bundle/"rain-letters-web"
web.mkdir(exist_ok=True)
for name in selected:
    target=web/name;target.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(source/name,target)
(bundle/"README.md").write_text('''# NIR 0.1.0 · Linux x86_64

解压后，在本目录运行：

```
./novelc serve rain-letters-web
```

在启用 WebGPU 的桌面 Chromium 打开 http://127.0.0.1:4173/ 。首次点击/按键解锁音频；空格继续，Esc 打开菜单。

创建新作品不需要 Rust：

```
./novelc init my-story
./novelc -p my-story resolve
./novelc -p my-story check --locked
./novelc -p my-story test
./novelc -p my-story dev
./novelc -p my-story build --locked
```

许可：LGPL-3.0-or-later；完整许可见 LICENSE、COPYING 和 LICENSE-NOTICE.md。对应源码随同版本源码包提供：https://github.com/iMMIQ/nir/releases 。

novelc 和 sdk 必须配套保留。最终作品默认输出到 my-story/dist/full/web，可完整上传至 HTTPS 静态站点，保留 NOTICE.txt。

能力边界见 docs/CAPABILITIES.md，实测结果见 docs/TEST-REPORT.md。真实 WebGPU 测试使用 SwiftShader 软件适配器；未宣称物理 GPU 或移动真机认证。
''')
def canonical(info):
    info.uid=info.gid=0;info.uname=info.gname="";info.mtime=0
    return info
def archive(path,root,paths,prefix):
    with path.open("wb") as file,gzip.GzipFile(filename="",mode="wb",fileobj=file,mtime=0) as gz,tarfile.open(fileobj=gz,mode="w") as tar:
        for p in sorted(paths):
            tar.add(p,arcname=str(Path(prefix)/p.relative_to(root)),recursive=False,filter=canonical)
archive(dist/"nir-0.1.0-linux-x86_64.tar.gz",bundle,[p for p in bundle.rglob("*") if p.is_file()],"nir-0.1.0")
root=Path(".")
# Match the audited Git source set; never recursively sweep local state into a release.
paths=source_files()
archive(dist/"nir-0.1.0-source.tar.gz",root,paths,"nir-0.1.0-source")
checks=[]
for name in ["nir-0.1.0-linux-x86_64.tar.gz","nir-0.1.0-source.tar.gz"]:
    path=dist/name;checks.append(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {name}")
    print(f"{name}: {path.stat().st_size:,} bytes")
(dist/"SHA256SUMS").write_text("\n".join(checks)+"\n")
