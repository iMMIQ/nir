# M4.1 / M4.2：发行、存档和桌面渲染

本轮直接扩展 release manifest v1，实现固定发行入口、存档隔离与双渲染后端。

## 构建与发行

```sh
dist/novelc -p examples/rain-letters resolve
dist/novelc -p examples/rain-letters build --profile release --out dist/candidate
dist/novelc release verify --directory dist/candidate
dist/novelc release stage --source dist/candidate --directory dist/site
# DIGEST 为候选发行 channels/stable.json 中的 release 字段。
dist/novelc release verify --directory dist/site --release DIGEST
dist/novelc release promote --directory dist/site --release DIGEST --expect none
```

`release` 构建强制使用锁文件；更新 SDK 后需显式重新 resolve。`dev` 构建允许解析 SDK，显示开发标识，并使用独立的玩家数据空间。localhost 上的 release 仍使用 release 空间。

`stage` 导入经过验证的对象、manifest 和固定启动文件，不改变频道。`promote` 在本地锁内检查预期当前发行、重新验证目标，再原子替换 `channels/stable.json`。已有不可变文件内容不一致时拒绝覆盖。

后续更新使用 `--expect OLD_DIGEST`。回滚命令为：

```sh
dist/novelc release rollback --directory dist/site --to OLD_DIGEST --expect CURRENT_DIGEST
```

回滚只切换新会话入口，不删除已发布文件和玩家存档。每次发行有固定的 `releases/DIGEST/index.html` 及 bootstrap；已启动会话不会跟随频道切换。

## 静态托管

1. 上传候选发行的不可变 `objects/` 和 `releases/` 文件，保留历史文件。
2. 在实际托管地址验证候选发行：`dist/novelc release verify --url https://example.org/story/ --release DIGEST`。
3. 准备根入口文件，最后使用托管平台提供的原子文件替换或部署快照切换频道。多位发布者必须串行操作或使用平台的条件写入。
4. 验证当前入口：`dist/novelc release verify --url https://example.org/story/`。

CLI 不接入云账号，不能代替远程平台的原子发布保证。可在根目录或子路径托管；URL 验证使用实际 GET，检查哈希、MIME、缓存和不存在资源的 404。对象、manifest、固定启动文件使用长期 immutable 缓存；根入口和频道必须重新验证或不缓存。不要配置将缺失资源回退成首页的 SPA 路由。

## 玩家数据

存档键由 GameId、profile、发行摘要和槽位组成；同发行的多标签写入使用 revision CAS。偏好和成就按 GameId/profile 共享。

发行存档入口列出各发行的存档，支持导出并打开相应固定发行入口，再从该版本读档。Core 继续要求存档发行身份完全匹配。目标必须属于同一站点、游戏和 profile，并通过 manifest 与启动文件校验；资源不可用时仍可导出存档。

开发数据使用新的 IndexedDB 空间 `nir-player-isolated-v1`，不读取、迁移或删除之前的开发数据库。清理历史托管文件会使对应发行无法继续游玩，发布流程不会自动清理它们。

## 渲染与验收入口

标准 WASM 同时包含 WebGPU 和 WebGL2。自动模式实际探测 WebGPU；初始化失败时更换画布后尝试 WebGL2。运行中的 GPU 恢复保持所选后端，失败显示重载入口。诊断报告记录实际 backend、适配器和 fallback_reason。

使用 `?test=1&backend=webgpu` 或 `?test=1&backend=webgl2` 强制测试后端；强制模式不回退。

```sh
bun run test:host
bun run test:browser
bun run test:backends
```

`test:backends` 覆盖 Chromium/WebGPU、Chromium/WebGL2、Firefox/WebGL2 的剧情、画布、音频、存档及设备恢复。可通过 `CHROMIUM`、`FIREFOX` 指定浏览器路径。Linux 无显示环境使用 Xvfb；软件渲染验收不代表实体 GPU、Safari 或移动端实机验收。

后续 M4.3 处理作者用例预览、开发实例隔离与独立作者交付验收。

## 本轮结果（2026-09-24）

- Rust 完整测试 171 项通过；随后新增预览回归所在 CLI 的 4 项测试、固定入口构建测试及渲染器 5 项测试通过。
- Host 57 项、现有浏览器回归分组共 45 项通过。
- 三组后端验收共 8 项通过，0 失败、0 跳过。涵盖真实 WebGL context loss、WebGPU 设备初始化失败后换画布回退，以及两个后端的颜色/透明混合对比。
- 无 PATH 的独立 SDK 通过发行导入、校验、晋级、回滚、损坏/缺失对象拒绝、CAS 冲突、错误 HTTP MIME/cache/404 和 dev 发行拒绝晋级测试。
- fmt、原生/WASM Clippy、架构检查与最终发行完整性验证通过。

本机使用 NixOS、Xvfb、Chromium 153 和 Firefox 155 的软件渲染。Firefox 的本地启动环境补齐了 GTK、GL 和 PulseAudio 动态库；CI 使用 Playwright 的系统依赖安装流程。具体计数、发行身份与剩余验收范围见 [验收摘要](validation/m4/summary.json)。
