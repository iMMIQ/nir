# Nix 开发环境

推荐在 Linux 上使用仓库的 Nix flake 开发引擎与 SDK。支持 `x86_64-linux` 和 `aarch64-linux`；这表示开发环境支持的架构，正式播放器发行平台仍以 [Linux 发布说明](LINUX.md) 为准。

## 进入环境

安装 Nix 后，在 `~/.config/nix/nix.conf` 的现有配置中启用 `experimental-features = nix-command flakes`。在仓库根目录执行：

```sh
nix develop
```

也可不改用户配置，单次开启所需功能：

```sh
nix --extra-experimental-features 'nix-command flakes' develop
```

环境提供 Git、rustup、GCC、libclang/bindgen、pkg-config、Python 3、Bun、Node.js 24、Chromium、ALSA、X11/Wayland、Vulkan 与 Xvfb。进入环境不会自动安装 JS 依赖或构建项目。

`flake.lock` 固定 Nix 软件包；`rust-toolchain.toml` 固定 Rust 及目标/组件，rustup 首次运行时需要联网下载；`.bun-version` 和 `package.json` 记录 Bun 版本；`Cargo.lock` 与 `bun.lock` 固定应用依赖。当前 wasm-bindgen CLI 仍需通过 Cargo 安装，版本必须与 Rust 依赖一致：

```sh
cargo install wasm-bindgen-cli --version 0.2.100 --locked
bun install --frozen-lockfile
cargo xtask sdk
```

若使用自定义 `CARGO_HOME` 或安装目录，构建 SDK 前设置 `WASM_BINDGEN` 为对应可执行文件的绝对路径；xtask 默认查找 `~/.cargo/bin/wasm-bindgen`。

日常验证：

```sh
cargo xtask test --quick
bun run test:host
```

只执行一条命令时可用 `nix develop --command cargo xtask test --quick`。退出交互环境使用 `exit`。

## 可选：自动加载

仓库包含 `.envrc`。在宿主环境安装 direnv 和 nix-direnv、完成对应 shell 集成后，在仓库根目录运行一次：

```sh
direnv allow
```

之后进入目录会自动加载环境，离开时恢复原环境。`.direnv/` 缓存已被 Git 忽略；不使用 direnv 时直接运行 `nix develop` 即可。

## 浏览器与原生渲染

开发环境将 `CHROMIUM` 指向 Nix 提供的 Chromium，常规浏览器测试和 `test:nir-next` 都使用该路径，无需运行 Playwright 的 Chromium 安装命令。进入环境前已设置的 `CHROMIUM` 会保留，也可以在环境中覆盖。完整多浏览器验收的 Firefox 仍需另行准备。

常规浏览器测试需要先按 [README](../README.md#从源码构建引擎与-sdk) 构建 SDK 与测试工程，再运行：

```sh
mkdir -p target/tmp
TMPDIR="$PWD/target/tmp" bun run test:browser
```

无桌面会话时可用 `xvfb-run -a` 包装测试命令；渲染验收仍须检查实际画面，限制见 README。`bun run test:nir-next` 使用无界面 Chromium 和 SwiftShader。

原生 Vulkan 默认沿用 lavapipe 软件渲染，便于无 GPU 环境验证。`VK_DRIVER_FILES` 可显式指定其他 Vulkan ICD；在已正确配置 GPU 驱动的主机上，也可在进入环境后运行 `unset VK_DRIVER_FILES`，恢复加载器的驱动发现。性能测量应使用目标硬件与相应驱动。

此环境用于本地开发。Nix 构建的原生程序可能引用 `/nix/store`，跨机器分发请遵循 [Linux 发布说明](LINUX.md)。Android NDK 不包含在此环境中，交叉编译配置见 [Android 发布说明](ANDROID.md)。

## 更新环境

日常使用提交到仓库的 `flake.lock`。需要升级 Nix 依赖时，显式运行 `nix flake update nixpkgs`，验证构建与测试后一起提交锁文件；若 Bun 版本发生变化，同步 `.bun-version` 与 `package.json`。

可用 `nix flake check --all-systems --no-build` 检查两个 Linux 架构的配置求值；实际编译和浏览器测试需要在对应主机执行。
