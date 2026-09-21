# NIR · 雨后书简

Rust / WASM / WebGPU 叙事引擎的可玩首版。示例包含简中、英文和两个结局。剧情推进、可视界面、文字排版和绘制均在 Rust 中；JavaScript 负责浏览器输入、文件获取、音频和本地存储。

这是六份 NIR 设计文档中一个受限能力集的实现，不代表六份规范的完整 V1。具体支持范围见 [能力表](docs/CAPABILITIES.md)，实际验证记录见 [验收报告](docs/TEST-REPORT.md)。

## 直接体验

从 [GitHub Releases](https://github.com/iMMIQ/nir/releases) 下载配套 SDK、Linux CLI 和静态作品，或按下文从源码构建。在源码构建目录运行：

```sh
./dist/novelc serve dist/rain-letters-web
```

打开 `http://127.0.0.1:4173/`，使用启用 WebGPU 的桌面 Chromium。远程静态托管须使用 HTTPS。直接双击 HTML（`file://`）不能运行播放器。

- 空格 / Enter：开始、显示整段、继续；Esc：菜单 / 返回。
- 鼠标或触摸：点击对白区、选项和绘制的按钮。
- Tab、方向键与 Enter：访问辅助语义按钮；可见焦点框对应画布按钮。
- 菜单提供回看、检查点回退、设置和三个存档槽。读档和设备恢复后先暂停，按继续恢复。
- 「已读快进」只推进已读正文，遇到未读段落或选项停止。自动阅读等待正文及当前语音完成。
- 声音须首次点击/按键解锁。`voice.wav` 是合成测试声，不是真人配音。

## 创建自己的作品

将 `dist/novelc` 与整个 `dist/sdk/` 放在同一目录，离开本仓库也能使用。编辑作品不需要安装 Rust。

```sh
./dist/novelc init /path/to/my-story
./dist/novelc -p /path/to/my-story resolve
./dist/novelc -p /path/to/my-story doctor
./dist/novelc -p /path/to/my-story check --locked
./dist/novelc -p /path/to/my-story test
./dist/novelc -p /path/to/my-story dev
./dist/novelc -p /path/to/my-story build --locked
```

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

## 从源码构建

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
TMPDIR="$PWD/target/tmp" npm run test:browser
python3 scripts/verify_release.py examples/rain-letters/dist/full/web
python3 scripts/verify_sdk.py
```

浏览器测试默认使用有窗口的 `/usr/bin/chromium`。可用 `CHROMIUM` 改路径，`NIR_CHROME_ARGS` 添加启动参数。Linux 无桌面环境可尝试 Xvfb，但必须实际检查画布截图；本机无界面模式曾出现 WebGPU 提交成功而画布空白的系统合成问题，不能把它当作通过。测试只在 `?test=1` 下启用只读状态及故障注入接口。

原始附件保存在 `docs/specs/`，仅作为设计依据。[架构决策](docs/ARCHITECTURE.md) 说明本实现的边界与取舍。代码采用 **LGPL-3.0-or-later**（GNU LGPL v3 或更高版本，见 [许可说明](LICENSE-NOTICE.md)）；原创示例图像/合成声音采用 CC0-1.0；字体及第三方代码保留各自许可。
