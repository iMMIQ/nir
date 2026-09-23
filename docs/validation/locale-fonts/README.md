# 独立语言上下文与逐语言字体验证

日期：2026-09-23。基于最终配套 SDK 和《雨后书简》静态发行，使用桌面 Chromium 153.0.8010.52 执行真实 WASM/WebGPU 回归。本次 WebGPU 适配器为 Google SwiftShader，`isFallbackAdapter = true`。**物理 GPU、移动真机及其他浏览器未验证**。

本阶段继续在 v1 研发格式内破坏性迭代。界面与正文各有支持语言、默认语言、玩家请求值和生效值；`config/locales.toml` 为每个语言指定有序 Font AssetId。旧单语言偏好会拆成两个语言偏好并保留音量；旧运行格式及不兼容存档严格拒绝。配音语言、SpeakerId／VoiceKey、远程正文包、Ruby 与多模块仍不在本阶段。

## 验证结果

| 检查 | 最终结果 |
|---|---|
| 格式与代码差异检查 | `cargo fmt --all --check`、`git diff --check` 通过 |
| 原生逻辑与依赖边界 | `cargo xtask test`：97 项通过；12 个包的实际依赖图检查通过 |
| 排版测试 | `cargo test -p nir-presentation --locked`：3 项通过 |
| 严格 lint | 原生与 WASM `-D warnings` 通过；vendored wgpu 自有的 2 条 unused-import 警告仍在 |
| 宿主测试 | 22 项通过，包含浏览器语言独立匹配和旧偏好迁移 |
| 独立 SDK | 无 Cargo 的 PATH 下完成 init／resolve／doctor／check／test／build；锁漂移拒绝、干净缓存重复构建一致 |
| 作者工程 | 《雨后书简》doctor／check／两条场景 test／locked build 通过，重复构建发行摘要一致 |
| 静态发行 | 13 个内容对象的大小和 SHA-256 全部校验通过 |
| 完整 Chromium | 25/25 通过，0 跳过、0 意外失败、0 重试通过；约 2.7 分钟 |
| 公开文件隐私扫描 | 通过；结果见 `privacy.json` |

核心与播放器回归验证独立语言请求和生效、连续切换的过时回调、字体资源失败后的取消与重试、候选预算租约、旧点击、读档／回退时冻结正文实例、设备与表面代次变化。排版测试验证缺少显式字体时先返回错误、不进入空字体库塑形，以及语言和 FontPlan 摘要隔离塑形缓存。编译器测试覆盖缺配置、未知字段、重复字体、非法默认语言、漏配正文、错误资源类型、缺字、字体许可、共享字体去重和重复构建一致性。

## 浏览器证据

- [中文菜单＋英文正文](locale-zh-ui-en-text.png)：实际画布使用中文 UI、英文对白和独立字体计划；辅助朗读语言为英文。
- [英文 UI 与冻结选项](locale-en-ui-frozen-en-choice.png)：切回中文正文后，已实例化的英文选项保持原语言和身份。
- [同屏双语回看](mixed-language-history.png)：中文旧句与英文新句分别使用冻结的文本和字体计划。
- [窄屏双语言设置](locale-narrow-settings.png)：UI／正文控件、当前生效值和返回动作均可见，标题没有遮挡。

[完整浏览器记录](browser-run.log)、[机器结果](browser-results.json)、[功能观测](browser-metrics.json)、[原生回归](native-tests.log)、[宿主回归](host-tests.log)、[独立 SDK](standalone-sdk.json) 和 [对象校验](example-release-verification.json) 保留了可复核证据。

## 产物身份和观测边界

SDK 摘要：`84f3d19562b1a440cd0520f49f57f7782b2adad4804ccd26f5b679caba90f95f`。

《雨后书简》发行摘要：`ec7adbcd18425e43b5b22589c06a1bf5e72cea3a5cd7d47928d070d99447b263`；13 对象，共 7,340,193 bytes。[作者构建报告](example-build.json)记录字体对象、字集和来源；发行目录由 `dist/rain-letters-web/` 提供，本地 SDK 和 CLI 位于 `dist/sdk/`、`dist/novelc`。

功能回归的单次观测：点击开始到首句约 101.4 ms，标题准备约 1,150.5 ms，全程提交 104 帧，资源账本峰值 67,169,732 bytes；静态标题等待时没有继续提交帧。该数据不是重复采样性能基准，账本也不是进程物理内存。SwiftShader 成绩不代表物理 GPU 成绩。

新增字体 ABeeZee 与 Noto Sans CJK SC 均随来源和 OFL-1.1 许可交付。现有 GitHub Release 下载资产未被替换。配置与使用方式见[语言和字体计划](../../LOCALE-FONTS.md)，支持边界见[能力表](../../CAPABILITIES.md)。
