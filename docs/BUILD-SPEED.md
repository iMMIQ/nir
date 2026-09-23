# 构建与测试速度

## 已实施

- 使用 Bun 1.3.13 管理 JS 依赖，版本与当前 `flake.lock` 的 Bun 一致。`.bun-version` 用于 CI，`package.json` 声明包管理器，`bun.lock` 从原 npm 锁迁移，Playwright 仍固定为 1.63.0。
- `bun install --frozen-lockfile` 安装锁定依赖；删除 `package-lock.json`，避免维护两份锁。添加依赖或升级时用 Bun，并提交更新后的锁文件。
- `bun run test:host` 使用 Bun 执行原有 22 项宿主测试。`bunfig.toml` 将默认 `bun test` 的范围限定到 `tests/host`。
- Playwright 经 `bun run test:browser` 启动，沿用其 Node.js 入口。Nix shell 提供 Bun 和 Node.js；浏览器依赖安装使用 `bunx --no-install playwright install --with-deps chromium`。
- CI 使用固定提交版本的 `Swatinem/rust-cache`，缓存 Rust 依赖、原生/WASM 依赖编译产物和 Cargo 安装的工具。主分支成功运行后保存缓存，PR 可恢复。安装 wasm-bindgen 前检查版本，命中 0.2.100 时直接复用。
- 完整 `cargo xtask test` 合并展示层测试，一次 Cargo 调用覆盖 113 项和架构检查。增加 `--quick` 和测试参数透传，方便缩短局部修改的反馈时间。

## 日常运行

```sh
nix develop
bun install --frozen-lockfile

# Core、资源、Player、展示层：70 项，无需构建字体编译器/CLI
cargo xtask test --quick
bun run test:host

# 定位特定测试
cargo xtask test --quick content::tests::restore_admission_failure_after_content_install_is_reported
bun run test:host --test-name-pattern 'shared fetch'
bun run test:browser tests/browser/modules.spec.js

# 提交前完整 Rust 检查；CI 始终运行完整入口
cargo xtask test
```

修改编译器、字体或 CLI 时直接运行对应包的测试或完整入口。快速入口不包含编译器/CLI 测试和架构检查，不能替代完整验收。

浏览器测试仍使用一个 worker：测试工程使用固定端口、共享 `dist` 输出，SwiftShader 也共享 CPU。增加 worker 前应先隔离这些资源并测量争用。

## 本机测量（2026-09-23）

环境：NixOS x86_64、Rust 1.98.1、Node.js 24.19.0、Bun 1.3.13。

| 项目 | 原方式 | 新方式 |
|---|---:|---:|
| 安装 JS 依赖，包缓存已热、每次清空 `node_modules` | npm ci：348ms | Bun frozen install：7ms |
| 原有 22 项宿主测试 | Node test runner：122ms | bun run test:host：55ms |

上述为各 5 次测量的中位数。安装测量两端均关闭生命周期脚本，npm 额外关闭 audit/fund；当前依赖没有所需的安装脚本。使用临时目录测量，项目内已有依赖未被删除。计时使用 Python `time.perf_counter()`，包含启动进程的时间。宿主测试的 Node 基准直接调用 `node --test tests/host/*.test.js`，未计入 npm 包装开销。

Rust 热构建单次测量：完整 113 项约 **15.50s**，快速 70 项约 **1.76s**。两者覆盖范围不同，差值表示局部迭代可省下的工作量；完整测试自身未宣称获得这个倍数的提速。首次验证时完整入口还花了 37.70s 编译依赖，总计 59.52s，说明编译缓存对总耗时影响明显。

原始本地测量保存在忽略目录 `reports/tooling-speed.json`、`reports/rust-test-speed.json` 和 `reports/rust-test-speed-warm.json`。这些热缓存数据不预测首次网络下载或 GitHub runner 的耗时。CI 缓存收益待实际运行测量，首次无缓存运行仍须完整编译。

## 验证

- 完整 Rust 113 项、快速 Rust 70 项、过滤测试入口通过。
- Bun 宿主 22 项通过；直接 `bun test` 也只运行这些测试。
- Playwright 可发现 28 项浏览器测试及 2 项性能测试；通过 Bun 实际执行三章节浏览器用例，3 项通过。
- frozen install、Rust 格式检查、xtask 严格 Clippy、Git diff 检查通过。
- CI YAML 通过 actionlint 和其 ShellCheck 检查；本轮未触发远程 CI。

依赖安装和宿主测试的绝对耗时已很小。后续若继续提速，优先测量字体编译测试、SDK 的原生链接与 wasm-bindgen 安装冷启动；再决定是否加入独立开发构建 profile 或隔离后的浏览器并发。

参考：[Bun 锁文件迁移](https://bun.sh/docs/pm/lockfile)、[Bun frozen install](https://bun.sh/docs/pm/cli/install)、[Bun 运行时与 Node shebang](https://bun.sh/docs/runtime)、[Rust Cache 缓存范围与键](https://github.com/Swatinem/rust-cache/tree/6323deb102c322ba6fcbdcafc7e3dddab59af2b6)。
