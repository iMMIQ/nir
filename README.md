# NIR 叙事引擎

NIR 是使用 Rust / WASM / WebGPU 实现的叙事引擎，提供浏览器播放器、作品开发工具 `novelc` 和可独立使用的 SDK。作者可以创建、检查、预览作品，并构建可部署到静态站点的发布目录；编辑作品无需重新编译引擎。

引擎负责确定性剧情执行、场景演出、文字排版与揭示、声音协调、存读档、检查点回退、回看和语言设置。剧情推进、可视界面、文字排版和绘制均在 Rust 中；JavaScript 负责浏览器输入、文件获取、音频和本地存储。模块职责与依赖边界见 [架构说明](docs/ARCHITECTURE.md)。

`examples/rain-letters/`（《雨后书简》）是随仓库提供的测试工程，用于验证引擎功能、运行两条剧情测试路线，以及演示作品工程格式。其简单图像和合成音频用于测试。

这是六份 NIR 设计文档中一个受限能力集的实现，不代表六份规范的完整 V1。具体支持范围见 [能力表](docs/CAPABILITIES.md)，v0.1.0 的验证记录见 [验收报告](docs/TEST-REPORT.md)，后续调度改进见 [引擎稳定性进展](docs/ENGINE-STABILITY.md) 与 [请求生命周期进展](docs/REQUEST-LIFECYCLE.md)。

## 使用 SDK 创建作品

从 [GitHub Releases](https://github.com/iMMIQ/nir/releases) 下载 Linux x86_64 SDK 与 CLI 包，解压后在包目录运行以下命令。保留 `novelc` 与整个 `sdk/` 在同一目录，离开本仓库也能使用。编辑作品不需要安装 Rust。

```sh
./novelc init my-story
./novelc -p my-story resolve
./novelc -p my-story doctor
./novelc -p my-story check --locked
./novelc -p my-story test
./novelc -p my-story dev
./novelc -p my-story build --locked
```

`init` 当前使用随 SDK 提供的测试工程模板，可在此基础上替换正文、逻辑和素材。从源码构建时，配套 CLI 与 SDK 位于 `dist/`。

`dev` 使用正式播放器和 4173 端口；修改内容后重新运行构建并刷新页面。当前没有文件监听或热更新。`dev --scenario tests/scenarios/walk.toml` 验证已登记的场景用例，再从合法新游戏入口打开预览，不会跳过剧情前置操作。

`game.lock` 固定 SDK 文件及配套 CLI 身份。显式更换 SDK 后运行 `resolve`；`--locked` 遇到漂移会报错。可以用 `--sdk /path/to/sdk` 或 `NIR_SDK` 指定 SDK，须使用该 SDK 配套的 `novelc`。

作品输出位于 `dist/full/web/`。完整上传该目录即可，可部署到子路径。保留 `NOTICE.txt`。部署时先上传对象和发行清单，最后更新 `channels/stable.json`；不要删除仍可能被旧会话引用的对象。源码目录、测试、源素材路径和本地配置不会作为运行目录复制进去。

## 作品结构

```text
game.toml                      作品身份、能力配置和输入清单
game.lock                      SDK 与 CLI 的真实内容身份
content/ch01/module.toml        唯一模块、入口、正文包
content/ch01/story.nir.json     变量、函数、块、Cue、场景、选项
content/ch01/texts/             文本契约及 zh-Hans / en 正文
assets/catalog.toml            资源身份、路径、类型、权利信息
assets/source/                 PNG、PCM16 WAV、OTF 字体
themes/rain/tokens.json         颜色主题
tests/scenarios/               按逻辑 ID 驱动的剧情用例
credits/                       素材许可
schemas/                       由 SDK 生成的 JSON Schema
```

通过正文包编辑对话，保留稳定 ID、文本修订、参数和 Gate 顺序。新增中文字符时需要更新许可合适的字体；`check` 会拒绝缺字。调整逻辑时参考示例块与 [编写说明](docs/AUTHORING.md)。

## 运行测试工程

下载包附带已构建的测试工程，在解压目录运行：

```sh
./novelc serve rain-letters-web
```

在源码构建目录则运行 `./dist/novelc serve dist/rain-letters-web`。打开 `http://127.0.0.1:4173/`，使用启用 WebGPU 的桌面 Chromium。远程静态托管须使用 HTTPS；直接双击 HTML（`file://`）不能运行播放器。

测试工程覆盖简中与英文正文、选项分支、场景与音频、存读档及回退等功能。运行方式与输入操作见 [测试工程说明](examples/rain-letters/README.md)，测试证据见 [验收报告](docs/TEST-REPORT.md)。

## 从源码构建引擎与 SDK

固定工具链在 `rust-toolchain.toml`，依赖锁在 `Cargo.lock` 和 `package-lock.json`。需要 Rust/rustup、Python 3、Node.js（仅浏览器测试）及桌面 Chromium。

```sh
rustup target add wasm32-unknown-unknown --toolchain 1.95.0
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
cargo test -p nir-presentation
cargo clippy --workspace --all-targets --exclude player-web --exclude nir-render-wgpu --exclude nir-platform-web -- -D warnings
cargo clippy -p player-web --target wasm32-unknown-unknown -- -D warnings
npm ci
npm run test:host
TMPDIR="$PWD/target/tmp" npm run test:browser
python3 scripts/verify_release.py examples/rain-letters/dist/full/web
python3 scripts/verify_sdk.py
```

浏览器测试默认使用有窗口的 `/usr/bin/chromium`。可用 `CHROMIUM` 改路径，`NIR_CHROME_ARGS` 添加启动参数。Linux 无桌面环境可尝试 Xvfb，但必须实际检查画布截图；本机无界面模式曾出现 WebGPU 提交成功而画布空白的系统合成问题，不能把它当作通过。测试只在 `?test=1` 下启用只读状态及故障注入接口。

原始附件保存在 `docs/specs/`，仅作为设计依据。[架构决策](docs/ARCHITECTURE.md) 说明本实现的边界与取舍。代码采用 **LGPL-3.0-or-later**（GNU LGPL v3 或更高版本，见 [许可说明](LICENSE-NOTICE.md)）；原创示例图像/合成声音采用 CC0-1.0；字体及第三方代码保留各自许可。
