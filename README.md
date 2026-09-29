# NIR 叙事引擎

NIR 是使用 Rust / WASM / WebGPU / WebGL2 实现的叙事引擎，提供浏览器播放器、作品开发工具 `novelc` 和可独立使用的 SDK。作者可以创建、检查、预览作品，并构建可部署到静态站点的发布目录；编辑作品无需重新编译引擎。

引擎负责确定性剧情执行、场景演出、文字排版与揭示、声音协调、存读档、检查点回退、回看和语言设置。剧情推进、作品可视界面、文字排版和绘制均在 Rust 中；JavaScript 负责浏览器输入、文件获取、音频、本地存储及发行存档管理。模块职责与依赖边界见 [架构说明](docs/ARCHITECTURE.md)。

`examples/rain-letters/`（《雨后书简》）是随仓库提供的测试工程，用于验证引擎功能、运行两条剧情测试路线，以及演示作品工程格式。其简单图像和合成音频用于测试。

这是六份 NIR 设计文档中一个受限能力集的实现，不代表六份规范的完整 V1。 当前处于研发阶段，协议在 v1 格式内迭代，CLI/SDK 配套使用。具体支持范围见 [能力表](docs/CAPABILITIES.md)，v0.1.0 的验证记录见 [验收报告](docs/TEST-REPORT.md)，后续调度改进见 [引擎稳定性进展](docs/ENGINE-STABILITY.md)、[请求生命周期进展](docs/REQUEST-LIFECYCLE.md) 、[诊断和性能测量](docs/DIAGNOSTICS.md) 、[作品配置和主题契约](docs/PROJECT-THEMES.md) 、[作者阅读和预览](docs/AUTHOR-READING.md) 、[字体编译和独立模板](docs/AUTHOR-FONTS.md) 与 [文本修订和翻译维护](docs/TEXT-REVISIONS.md)。

## 使用 SDK 创建作品

从 [v0.1.1 Release](https://github.com/iMMIQ/nir/releases/tag/v0.1.1) 下载 Linux x86_64 SDK 与 CLI，或按下文从源码构建，在包目录运行以下命令。保留 `novelc` 与整个 `sdk/` 在同一目录，离开本仓库也能使用。编辑作品不需要安装 Rust。

构建产物为 `dist/novelc` 与 `dist/sdk/`；可以将它们一起复制到独立目录交给作者使用。

```sh
./novelc init my-story
./novelc -p my-story resolve
./novelc -p my-story doctor
./novelc -p my-story check --locked
./novelc -p my-story test
./novelc -p my-story dev
./novelc -p my-story build --locked
```

当前源码的 `init` 默认创建独立的最小双语作品，附完整母版字体，新增中文后由 CLI 自动裁剪。`init --template web-basic` 可创建《雨后书简》功能回归模板。已发布的 v0.1.0 仍使用旧测试工程模板。从源码构建时，配套 CLI 与 SDK 位于 `dist/`。

`dev` 使用正式播放器和 4173 端口；监听作者文件，构建成功后自动完整重载，失败时保留上次有效预览并显示诊断。重载从标题入口开始，未保存进度会丢失；不迁移旧发行会话。详见 [长内容与开发预览](docs/AUTHOR-READING.md)。`dev --scenario tests/garden.toml` 验证已登记的场景用例，再从合法新游戏入口打开预览，不会跳过剧情前置操作。

`game.lock` 固定 SDK 文件及配套 CLI 身份。显式更换 SDK 后运行 `resolve`；`--locked` 遇到漂移会报错。可以用 `--sdk /path/to/sdk` 或 `NIR_SDK` 指定 SDK，须使用该 SDK 配套的 `novelc`。

作品输出位于 `dist/full/web/`。完整上传该目录即可，可部署到子路径。保留 `NOTICE.txt`。部署时先上传对象和发行清单，最后更新 `channels/stable.json`；不要删除仍可能被旧会话引用的对象。源码目录、测试、源素材路径和本地配置不会作为运行目录复制进去。

## 外部引擎导入

SDK 的 `novelc` 内置 Rust 导入工具，无需 Python。当前实验性支持 LiveMaker LSB 116 的结构检查、基础控制流和同步对白迁移；识别到已适配的 LiveNovel 配置时，还可迁移事件、原图标题／回想菜单、GAL 图像及 WAV/Ogg 声音。未支持的语义会报告阻塞项，原引擎效果的替代会列入报告。运行 `novelc import inspect <游戏目录>` 检查原包，或 `novelc import livemaker <游戏目录> --out <新工程目录>` 转换。具体范围、草稿模式和限制见 [外部引擎导入](docs/IMPORT.md)。

## Windows 原生发布

在 Windows x64 构建 `cargo xtask sdk` 后，运行 `novelc -p my-story resolve` 和 `novelc -p my-story build --target windows --locked`。分发完整 `dist/full/windows/` 目录，玩家双击 `Game.exe`，无需浏览器或本地服务器。使用 DirectX 12 原生渲染，详见 [Windows 发布与验证](docs/WINDOWS.md)。

## Linux 原生发布

在 Linux x86_64 构建 `cargo xtask sdk` 后，运行 `novelc -p my-story resolve` 和 `novelc -p my-story build --target linux --locked`。分发完整 `dist/full/linux/` 目录（打包为 tar.gz），玩家运行 `./Game`，无需浏览器或本地服务器。使用 Vulkan 原生渲染，详见 [Linux 发布与验证](docs/LINUX.md)。

## Android 原生发布（实验性）

SDK 含 NDK 交叉编译的 `sdk/android/lib/arm64-v8a/libplayer.so` 时，作者运行 `novelc -p my-story build --target android --locked` 即得到可直接安装的 `dist/full/android/Game.apk`（Android 8.0+ arm64，Vulkan 渲染），全程无需 Java、Gradle 或 Android SDK：清单、打包与 v2 签名由 novelc 内置完成。签名密钥自动生成于作品 `config/android-signing.pem`，更新安装必须保留。当前为实验性：构建与签名经结构验证，真机验收见 [Android 发布与验证](docs/ANDROID.md)。

## 作品结构

```text
game.toml                      作品身份、能力配置和输入清单
config/locales.toml            UI/正文语言、默认值和逐语言字体计划
game.lock                      SDK 与 CLI 的真实内容身份
content/main/module.toml        模板入口模块、导出、正文包
content/main/story.nir.json     变量、函数、块、Cue、场景、选项
content/main/texts/             文本契约、修订记录及 zh-Hans / en 正文
assets/catalog.toml            资源身份、路径、类型、权利信息
assets/fonts/                  字体母版、来源和许可（可另加 PNG、PCM16 WAV）
config/player.toml             可选：作品默认设置，需在 inputs.player 登记
theme/theme.toml               主题与对白/选项组件绑定
theme/tokens.json              颜色主题
tests/                         按逻辑 ID 驱动的剧情用例
NOTICE.md                      模板许可
schemas/                       由 SDK 生成的 JSON Schema
```

本分支构建的 SDK 支持 `novelc -p my-story config` 查看配置值和来源；主题及默认设置编辑说明见 [作品配置](docs/PROJECT-THEMES.md)，独立 UI/正文语言及字体计划见[语言说明](docs/LOCALE-FONTS.md)。已发布的旧版 SDK 不会自动获得新增能力。

多章节作品可登记多个模块和共享变量，通过显式导出跨模块调用。发行包按模块拆分函数体，按模块与语言拆分正文，播放器在执行、切换语言和读档时按需获取。作者配置、并行实现流程及当前边界见 [多模块工作流](docs/MODULE-WORKFLOW.md)。

通过正文包编辑对话，保留稳定 ID、参数和 Gate 顺序。源文修改后执行 `text update --id <TextId> --meaning preserve|bump`，确认译文后执行 `text review --id <TextId> --locale en`；用 `text status` 查看待复核项。旧工程需显式迁移，见 [文本修订说明](docs/TEXT-REVISIONS.md)。最小模板会自动从母版生成所需字形；新字符超出母版覆盖时 `check` 会拒绝缺字，配置与边界见 [字体编译](docs/AUTHOR-FONTS.md)。调整逻辑时参考示例块与 [编写说明](docs/AUTHORING.md)。

## 运行测试工程

下载包附带已构建的测试工程，在解压目录运行：

```sh
./novelc serve rain-letters-web
```

在源码构建目录则运行 `./dist/novelc serve dist/rain-letters-web`。打开 `http://127.0.0.1:4173/`，使用启用 WebGPU 的桌面 Chromium。远程静态托管须使用 HTTPS；直接双击 HTML（`file://`）不能运行播放器。

测试工程覆盖简中与英文正文、选项分支、场景与音频、存读档及回退等功能。运行方式与输入操作见 [测试工程说明](examples/rain-letters/README.md)，测试证据见 [验收报告](docs/TEST-REPORT.md)。

## 从源码构建引擎与 SDK

**推荐使用项目级 Nix 开发环境**，适用于 NixOS 及安装了 Nix 的 Linux（x86_64 / aarch64）。仓库的 `flake.nix` 声明开发依赖，`flake.lock` 固定其版本，无需逐项安装系统编译库。当前未提供 macOS 或 Windows 的 Nix 开发环境。

安装 Nix 并启用 `nix-command` 和 `flakes` 后，在仓库根目录运行（完整说明见 [Nix 开发环境](docs/NIX.md)）：

```sh
nix develop
bun install --frozen-lockfile
```

开发环境提供 Git、GCC、libclang、rustup、Python、Bun、Node.js、Chromium、ALSA 和 X11/Wayland/Vulkan 库，自动配置 bindgen 与浏览器路径。Rust 版本仍由 `rust-toolchain.toml` 控制，首次构建时 rustup 会下载所需工具链；wasm-bindgen CLI 按下面的固定版本安装。也可使用 `nix develop --command cargo b` 单次构建，或通过仓库的 `.envrc` 配合 direnv 自动进入环境。

手动配置环境时，按 `rust-toolchain.toml` 和 `.bun-version` 安装 Rust/rustup 与 Bun，并准备 Python 3、C++ 编译器、libclang（Ubuntu：`g++ libclang-dev`）、Node.js（Playwright 运行时）、Chromium 及 Linux 原生音频依赖（Ubuntu：`libasound2-dev`）。Rust 与 JS 依赖分别由 `Cargo.lock` 和 `bun.lock` 固定。

```sh
rustup target add wasm32-unknown-unknown --toolchain 1.98.1
cargo install wasm-bindgen-cli --version 0.2.100 --locked
cargo xtask sdk
./dist/novelc -p examples/rain-letters resolve
./dist/novelc -p examples/rain-letters build --locked
./dist/novelc -p examples/rain-letters build --locked --out dist/rain-letters-web
```

`cargo xtask sdk` 构建实际 WASM、绑定、平台宿主、原生 CLI、第三方许可、模板和 Schema。使用 wgpu 25.0.2、glyphon 0.9.0、cosmic-text 0.14.2。wgpu 维护一个经过真实设备丢失测试的 [兼容性补丁](vendor/README.md)。

```sh
cargo fmt --all --check
cargo xtask test
cargo clippy --workspace --all-targets --exclude player-web --exclude nir-render-wgpu --exclude nir-platform-web -- -D warnings
cargo clippy -p player-web --target wasm32-unknown-unknown -- -D warnings
bun install --frozen-lockfile
bun run test:host
TMPDIR="$PWD/target/tmp" bun run test:browser
python3 scripts/verify_release.py examples/rain-letters/dist/full/web
python3 scripts/verify_sdk.py
```

日常修改 Core/Player/展示层可先运行 `cargo xtask test --quick` 和 `bun run test:host`；编译器、字体和 CLI 的测试及架构检查仍由完整 `cargo xtask test` 执行。还可用 `cargo xtask test --quick <测试名>` 或 `bun run test:browser tests/browser/modules.spec.js` 定位验证。CI 保留完整测试，并缓存 Rust 依赖编译产物和固定版本的 wasm-bindgen CLI。迁移说明和实测结果见 [构建与测试速度](docs/BUILD-SPEED.md)。

浏览器测试默认使用有窗口的 Chromium；Nix 环境自动设置 `CHROMIUM`，环境外默认路径为 `/usr/bin/chromium`。可用 `CHROMIUM` 改路径，`NIR_CHROME_ARGS` 添加启动参数。Linux 无桌面环境可尝试 Xvfb，但必须实际检查画布截图；本机无界面模式曾出现 WebGPU 提交成功而画布空白的系统合成问题，不能把它当作通过。测试只在 `?test=1` 下启用只读状态及故障注入接口。

发行晋级、回滚、存档隔离和 WebGPU/WebGL2 验收见 [M4 发行与桌面渲染](docs/M4-RELEASE.md)。

原始附件保存在 `docs/specs/`，仅作为设计依据。[架构决策](docs/ARCHITECTURE.md) 说明本实现的边界与取舍。代码采用 **LGPL-3.0-or-later**（GNU LGPL v3 或更高版本，见 [许可说明](LICENSE-NOTICE.md)）；原创示例图像/合成声音采用 CC0-1.0；字体及第三方代码保留各自许可。
