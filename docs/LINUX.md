# Linux 原生发行

Linux 版使用 Rust、winit、wgpu Vulkan 和原生音频输出（ALSA/PulseAudio，经 rodio）。它与 Web 版和 Windows 版共用 `nir-engine`、Player、Core、文字排版和场景合成（`apps/player-desktop` 为共享桌面外壳，`apps/player-linux` 为平台入口），不使用浏览器、JavaScript、WASM 或本地 HTTP 服务。

## 构建

在 Linux x86_64（参考环境 Ubuntu 24.04）上安装仓库固定 Rust 工具链、C++ 编译器、libclang、Python 3、`libasound2-dev` 和 wasm-bindgen 0.2.100，然后构建配套 SDK：

```sh
cargo xtask sdk
./dist/novelc -p my-story resolve
./dist/novelc -p my-story build --target linux --locked
```

更换 SDK 后显式执行 `resolve`；Web 与 Windows 构建命令保持不变。`--out` 可指定其他 Linux 输出目录，`--profile dev` 隔离开发存档。

Linux SDK 增加 `sdk/linux/player-linux`，真实字节纳入 `game.lock`。不含此文件的 SDK 明确拒绝 Linux 构建（`E_SDK_LINUX`）。已有配套 Linux 播放器的 SDK 可以在其他主机打包内容；原生播放器自身需要 Linux 构建环境。注意：可执行位依赖打包主机的文件系统——在 Windows 主机上打包 Linux 版时压缩包不携带 0755，终端用户需先执行 `chmod +x Game`（`verify_linux.py` 也会因此失败）；请在 Linux/macOS 主机打包，或使用保留模式的 tar 格式。

输出 `dist/full/linux/` 包含 `Game`、`data/release.txt`、`data/releases/<digest>.json`、`data/objects/`、`NOTICE.txt` 和 `README.txt`，`Game` 保留可执行位。将整个目录打包为 tar.gz 分发，解压后运行 `./Game`。不要只发送单个二进制。

当前支持 x86_64 Linux（构建环境 Ubuntu 24.04，glibc 2.39）与支持 Vulkan 的驱动；音频需要 ALSA 或 PulseAudio（缺失时播放器静默降级，不影响运行）。X11 与 Wayland 均可。暂不提供 AppImage、deb/snap 包装、代码签名、自动更新或 ARM64 构建。

编译器在作者侧 `.nir/linux-content/` 复用既有内容编译流程，再只发布原生需要的对象。最终 Linux 包没有网页启动文件或 WASM。原生清单记录播放器摘要；启动校验二进制，资源按需读取并验证摘要。`./Game --verify` 校验完整内容图。

存档导入/导出使用 XDG 文件对话框（xdg-desktop-portal，或回退 zenity）；无桌面会话时视为取消。致命启动错误输出到 stderr。

## 使用与存档

鼠标选择界面按钮；空格/Enter 开始、继续或推进；Esc 打开/关闭菜单；数字 1–3 选择对应选项；滚轮滚动正文/选项/回看；F11 全屏。窗口失去焦点时暂停剧情和音频。

玩家数据位于 `$XDG_DATA_HOME/NIR/games/<game-id-hash>/<profile>/`（默认 `~/.local/share/NIR/games/...`），与游戏安装路径无关，可用 `--data-dir` 覆盖。存档进一步按原生发行摘要隔离；Web 和原生存档不共享，不做跨发行迁移。继续旧存档需要保留旧游戏目录。

存档在临时文件刷盘后原子替换，检查槽位修订。每个作品/profile 持有进程文件锁，避免两个实例覆盖存档；进程结束后锁自动释放。

资源读取和 WAV 解码在单个后台线程执行，待处理队列有界；GPU 上传按每轮 2 MiB 分步完成。音频支持总线音量、循环、暂停、恢复和真实完成事件。GPU 丢失沿共用引擎恢复流程重新创建设备。

## 验证

```sh
cargo test -p player-desktop --lib
cargo check -p player-web --target wasm32-unknown-unknown
python3 scripts/check_architecture.py
python3 scripts/verify_linux.py my-story/dist/full/linux
```

自动验收使用隔离测试存档目录（`--hidden --data-dir`），推进剧情并验证存读档；报告写入 `reports/linux/`。CI 在 Xvfb 与 lavapipe 软件 Vulkan 上运行真实原生渲染。测试开关不改变发行身份。
