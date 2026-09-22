# 文本修订与翻译维护验证

日期：2026-09-22。基于本阶段最终 CLI/SDK，使用 Chromium 153.0.8010.52、Xvfb/X11 和 SwiftShader 执行真实 WASM/WebGPU。适配器 `fallback = true`；未验证物理 GPU、移动真机或其他浏览器。

按研发阶段约定，**在 v1 上破坏性迭代**：Program、Executable、Snapshot 均保持版本 1，能力标识为 `text.revisions.v1`。旧结构因字段不兼容拒绝加载；仍要求精确发行匹配，不提供旧存档迁移。

## 验证结果

| 检查 | 结果 |
|---|---|
| 格式、diff 空白检查 | 通过 |
| `cargo xtask test` | 90 项通过，实际依赖边界通过 |
| `cargo test -p nir-presentation --locked` | 1 项通过 |
| 原生与 WASM clippy `-D warnings` | 通过；vendor/wgpu 仍有既有 2 项警告 |
| 宿主调度/存储测试 | 20 项通过 |
| `cargo xtask sdk` | 实际 CLI、WASM、双模板、Schema 和许可构建成功 |
| 独立 SDK | 空 PATH 的最小工程 init/resolve/check/test/build、源文更新、过期阻断、复核、字体生成通过 |
| 确定性与锁定 | 同一工程重复构建、删除字体缓存后再构建的发行摘要一致；SDK 漂移拒绝 |
| 旧工程迁移 | 原目录不变，独立目录保留 GameId/TextId/语义修订；两条路线与构建通过 |
| 完整 Chromium 回归 | 24 项通过，317.4 秒；无跳过或重试通过 |
| 两份静态发行 | 全部对象 SHA-256 与大小校验通过 |
| 公开源码隐私扫描 | 通过，见 privacy.json；CLI/JS/WASM 无开发者绝对路径 |

原生新增用例覆盖：普通修订保留契约摘要及已读键、显式含义变更产生新已读键、Gate/参数变化强制 bump、漏参与重排 Gate 拒绝、缺失与多余文本、未知字段、仅抄版本号不能代替复核、译文本身修改后重新复核、新文本登记、无字节变化的语义更新、整数溢出无部分写入、事务恢复拒绝覆盖额外编辑、旧工程迁移和目标目录限制、运行契约摘要篡改、旧文本/快照结构拒绝、准备中对白冻结与历史身份恢复。

诊断位置取自同一份已验证字节；文本事务日志的出现/消失进入 dev 监听，其他缓存仍被忽略。预览单测和真实浏览器验证恢复后即使正文没有变化也能重新构建。

## 浏览器作者流程

新增用例从 SDK 创建最小作品并启动正式 dev：修改源文后拒绝候选并保留当前画面；登记修订后报告英文过期/未复核；修改并 review 后完整重载，显示新中文。切换语言保留当前对白，保存/刷新/读档仍保留该中文实例，重新开始后显示已复核英文。复核工具不自动修改翻译语义。

- [状态报告](stale-status.json)：逐 TextId/locale 的过期和待复核原因，包含项目内路径和 JSON Pointer。
- [构建被阻止时的原画面](text-stale-preview.png)：原中文对白保留，开发诊断可见。
- [复核后的英文](text-reviewed-english.png)：真实画布显示新译文和英文界面。
- [完整浏览器记录](browser.log) 与 [机器结果](browser-results.json)。

首次新增浏览器用例在短句已自然揭示完毕后又发送了推进，误走到选项；测试改为等待揭示完成，随后单独回归及最终完整 24 项均通过。正式播放器没有为该用例更改推进语义。

## 产物身份

SDK：`ebfa48555ffd6073a16aaae33380b25765b1143ace714f0eadbced64854f2df1`。

测试工程发行：`9ae9ea41eeb2ec66e97a2bcf6203e3c98ecb839f8e49c3f0f7d759929e5f0531`，12 个对象、7,219,745 bytes。

迁移后的最小作品发行：`aeaf41dbcc76a24d2e8fe6b529bfba224a3a889006bce1e086919ee1490c4f8a`，6 个对象、6,572,089 bytes。见 [产物格式和迁移检查](artifacts.json)、[独立 SDK](standalone.json)、[测试工程对象校验](release.json) 和 [最小作品对象校验](minimal-release.json)。

本地交付：`dist/novelc`、`dist/sdk/`、`dist/rain-letters-web/`、`dist/revision-story/` 和 `dist/revision-web/`。旧 `dist/my-story/` 保留为原格式。没有更新 GitHub Release 或替换 v0.1.0 下载资产。

## 观测与边界

功能运行单次观测：点击开始到首句 177.5 ms，导航到首句 4147 ms（含测试等待和输入），场景共提交 88 帧，资源账本峰值 67,176,404 bytes；标题静态等待期间帧数不增加。见 [观测记录](browser-metrics.json)。这不是新的重复采样性能基准，也不是物理 GPU 成绩；资源账本不等同于进程内存测量。

本阶段不包含独立 UI/正文/语音 LocaleContext、SpeakerId/VoiceKey 目录、多模块或跨发行存档迁移。修订记录不是签名或防篡改机制；迁移仅将合法旧译文导入为初始基线，不能证明历史人工复核。多文件写入具备中断检测和显式回滚，没有断电持久性或多人并发编辑认证。用法与限制见 [文本修订说明](../../TEXT-REVISIONS.md)。
