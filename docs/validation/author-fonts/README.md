# 字体编译与最小模板验证

日期：2026-09-21。测试基于本阶段最终 SDK，桌面 Chromium 153.0.8010.52，通过 Xvfb/X11 与 SwiftShader 执行真实 WASM/WebGPU。软件适配器 `fallback = true`；未验证物理 GPU、移动设备、其他浏览器或其他原生发行平台。

## 结果

| 检查 | 结果 |
|---|---|
| `cargo fmt --all --check`、`git diff --check` | 通过 |
| `cargo xtask test` | 81 项通过，实际依赖边界通过 |
| `cargo test -p nir-presentation --locked` | 1 项通过 |
| 原生与 WASM clippy `-D warnings` | 通过；vendor/wgpu 仍有既有 2 项警告 |
| 宿主调度/存储测试 | 20 项通过 |
| `cargo xtask sdk` | 原生 CLI、WASM、许可、双模板及 Schema 构建成功 |
| 独立 SDK 流程 | PATH 为空时最小模板 init/resolve/check/test/build、新增汉字、清空缓存重复构建通过；SDK 漂移拒绝 |
| 完整 Chromium 回归 | 23 项通过，约 2.5 分钟 |
| 两份静态发行对象校验 | 《雨后书简》和最小作品均通过 |
| 源码隐私扫描、CLI 路径检查 | 通过；扫描范围与结果见 privacy.json |

原生字体测试覆盖：冷/热缓存一致性、损坏缓存重建、删除缓存再现、字集变化、标题和 Assign String 常量、动态预留、full 全字符保留、许可证缺失/空值/越界、未知配置、非法 face/字体、缓存目录符号链接、TrueType 与 TTC face 提取、AAT 拒绝、许可变更影响发行但复用字体。塑形对比检查连字、组合重音及汉字的 glyph 数量、cluster、advance、offset 和轮廓边界，检查输出 GSUB/GPOS/GDEF 表。用例同时验证自定义变量断言成功与失败。

最小模板通过两条用例，与示例 affection 无耦合；`web-basic` 的旧用例继续通过。浏览器新增测试从独立工程编译“春夏秋冬，雪山与鲸鱼”，通过正式发布目录加载字体与中文、完成两条路线、切换英文和窄屏、保存/刷新/读档。dev 中插入母版不支持的 emoji 会保留上一发行和对白，修复为新汉字后编译字体并完整重载。

截图人工检查发现并修复了硬编码 `↗` 被漏裁的问题，随后以最终 SDK 跑完全部 23 项。控件符号已移到 Fluent 消息，加入覆盖断言。最终截图：

- [新增中文](fonts-chinese.png)：包括新字与右下角箭头，正常绘制。
- [窄屏英文](fonts-english.png)：英文换行及菜单正常。
- [开发预览更新](fonts-preview.png)：修复缺字后的新字体成功加载。

## 产物身份

SDK：`166b438ce4ab2fc7d75eb41350b66e3cf2f875a28db5d0d53eeefff34ff9456c`。

《雨后书简》发行：`9a26cc79e408c80f20bc1c800d535b87a2e78397c2729139e489064c9d695e62`，12 对象、7,226,613 bytes。最小作品发行：`6c92bd125354a814eb2bafc4b702bb994e64ae54383bade4467d5e9ec9fd5872`，6 对象、6,581,498 bytes。独立 init 的 GameId 是随机新身份，因此另一个新工程的发行摘要会不同；同一工程重复构建测试要求相同摘要。

母版 Noto Sans CJK SC：16,437,364 bytes。最小作品运行字体：115,376 bytes，235 个 cmap 字符；加入浏览器测试的新汉字后为 118,152 bytes。母版不在运行对象清单。分别见 [最小作品报告](minimal-build.json)、[新增汉字报告](fonts-author-build.json)。

本地交付 `dist/novelc`、`dist/sdk/`、`dist/my-story/`、`dist/minimal-web/` 与 `dist/rain-letters-web/`。没有上传新 GitHub Release 或替换 v0.1.0 资产。

## 测量与边界

功能回归单次观测中，点击开始到首句约 150.6 ms；导航到首句 2738 ms（包含测试刻意等待/输入时间）；场景全程提交 94 帧，资源账本峰值 67,176,404 bytes。标题静态等待 600 ms 的帧数不变。见 [观测记录](browser-metrics.json)。这些是软件 GPU 功能运行的观测值，不是新性能基准；账本估算不是物理内存测量。没有重新执行重复采样的性能基准，也没有声称解决既有软件渲染 P95 帧耗时问题。

目前仅认证这些静态字体和本测试的塑形样本。未实现逐语言 fallback/FontPlan、独立塑形 locale、动态补字、可变/彩色字体、完整 RTL/Ruby 支持、缓存淘汰或跨工程缓存。full 模式通过原生字形覆盖测试，完整 16 MB 字体的浏览器内存/性能未单独认证。
