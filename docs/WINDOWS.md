# Windows 原生发行

Windows 版使用 Rust、winit、wgpu DirectX 12 和原生音频输出。它与 Web 版共用 `nir-engine`、Player、Core、文字排版和场景合成，不使用浏览器、WebView2、JavaScript、WASM 或本地 HTTP 服务。

## 构建

在 Windows x64 上安装仓库固定 Rust 工具链、Visual Studio C++ Build Tools、LLVM/libclang、Python 3 和 wasm-bindgen 0.2.100，然后构建配套 SDK：

```powershell
cargo xtask sdk
.\dist\novelc.exe -p target/convert/yuuko2 resolve
.\dist\novelc.exe -p target/convert/yuuko2 build --target windows --locked
```

更换 SDK 后显式执行 `resolve`；Web 默认构建命令保持不变。`--out` 可指定其他 Windows 输出目录，`--profile dev` 隔离开发存档。

Windows SDK 增加 `sdk/windows/player-windows.exe`，真实字节纳入 `game.lock`。不含此文件的 SDK 明确拒绝 Windows 构建。已有配套 Windows 播放器的 SDK 可以在其他主机打包内容；原生播放器自身需要 Windows 构建环境。

输出 `dist/full/windows/` 包含 `Game.exe`、`data/release.txt`、`data/releases/<digest>.json`、`data/objects/`、`NOTICE.txt` 和 `README.txt`。将整个目录压缩为 ZIP 分发，解压后双击 `Game.exe`。不要只发送 EXE。

当前支持 Windows 10/11 x64 和支持 DirectX 12 的驱动，无需 Rust、SDK、浏览器或网络。暂不提供安装器、代码签名、自动更新、x86/ARM64 构建。

编译器在作者侧 `.nir/windows-content/` 复用既有内容编译流程，再只发布原生需要的对象。最终 Windows 包没有网页启动文件或 WASM。原生清单记录播放器摘要；启动校验 EXE，资源按需读取并验证摘要。`Game.exe --verify` 校验完整内容图。

## 使用与存档

鼠标选择界面按钮；空格/Enter 开始、继续或推进；Esc 打开/关闭菜单；数字 1–3 选择对应选项；滚轮滚动正文/选项/回看；F11 全屏。窗口失去焦点时暂停剧情和音频。

玩家数据位于 `%LOCALAPPDATA%/NIR/games/<game-id-hash>/<profile>/`，与游戏安装路径无关。存档进一步按原生发行摘要隔离；Web 和原生存档不共享，不做跨发行迁移。继续旧存档需要保留旧游戏目录。

存档在临时文件刷盘后原子替换，检查槽位修订。每个作品/profile 持有进程文件锁，避免两个实例覆盖存档；进程结束后锁自动释放。导入/导出使用系统文件对话框。暂不提供 Web 的发行存档历史面板，也未接入 Windows 屏幕阅读器的 UI Automation 语义桥。

资源读取和 WAV 解码在单个后台线程执行，待处理队列有界；GPU 上传按每轮 2 MiB 分步完成。音频支持总线音量、循环、暂停、恢复和真实完成事件。GPU 丢失沿共用引擎恢复流程重新创建设备。

## 验证

```powershell
cargo test -p player-desktop --lib
cargo check -p player-web --target wasm32-unknown-unknown
python scripts/check_architecture.py
python scripts/verify_windows.py target/convert/yuuko2/dist/full/windows
```

自动验收使用隔离测试存档目录，运行真实原生 GPU，推进剧情并验证存读档；报告写入 `reports/windows/`，默认隐藏测试窗口。测试开关不改变发行身份。
