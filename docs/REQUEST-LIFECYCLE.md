# 请求终态预留与竞态验证

此阶段继续完善 NIR 引擎的宿主协议。测试工程只用于触发资源、剧情、音频与存储行为，不改变引擎作为项目主体的定位。

## 行为变化

每个异步请求在启动前预留完成槽，进度复用该槽，完成、失败或取消后释放。重复完成不会二次进入引擎。普通工作最多占 248 项，控制工作可使用剩余 8 项，总容量仍为 256；输入受 128 项水位限制。资源仍共用四个物理执行名额。

保存等待 IndexedDB 的事务完成通知，即使场景会话已替换仍可报告结果。读档、导入与发起会话绑定，旧候选的迟到结果不能进入新会话。打开新的候选请求会取消旧候选。返回标题后，循环音频、准备工作和候选读档的逻辑槽均应释放。

设备丢失在每次 owner 运行前检查；恢复期间保留普通回执，新设备准备好后继续消费。静态页面仍有设备巡检，画面没有变化时不会额外提交 GPU 帧。

协议细节见 [ADR-008](ARCHITECTURE.md#adr-008异步请求的终态预留)。

## 验证范围

宿主测试覆盖满容量下的已接纳回执、控制保留容量、分片进度、取消与重复回调、关闭时结算，并用固定种子进行 20,000 次事件交错，逐步断言容量上限及 `accepted = completed + cancelled + active`。

浏览器测试延迟 IndexedDB 事务通知，以真实事务模拟旧回执；同时反复执行准备、暂停、设备恢复、返回标题，检查活动槽归零。设备恢复回归延后空闲巡检，单独验证 owner 能先发现失效设备。

测试日期：2026-09-21。最终静态发行身份 `3922d0e53cd627c40e1913420ff3129f7007d346f561a215722eb1ae1b630539`，12 个对象，共 6,735,067 字节。SDK 身份 `64bdf15bd475f8ca5639fa9e8a40610797cabe1d492ecc94b51a2acd547ef707`。

| 检查 | 结果 |
|---|---|
| 原生测试、依赖边界 | 55 项通过 |
| 宿主测试 | 14 项通过，含固定种子 20,000 次交错 |
| 真实浏览器 | 16 项通过；0 失败、跳过或 flaky；81.9 秒 |
| 格式、原生/WASM lint | 通过，第三方 wgpu 原有 unused-import 警告仍在 |
| 独立 SDK、锁漂移、重复构建 | 通过；相同输入生成同一发行 |
| 发布对象完整性 | 通过 |

浏览器为 Chromium 153.0.8010.12，在 1920×1080 虚拟屏幕内以有窗口模式运行。适配器为 SwiftShader 软件 WebGPU，测试实际 WASM、中文画布像素与设备销毁恢复，未验证物理 GPU。

12 轮交错运行包含 3 次设备重建，累计接纳 170 个请求：152 个完成、18 个取消、0 个残留；无资源失败，最大同时预留 6 个槽。该运行的恢复后帧数与上传计数是当前设备计数，不能解释为跨设备总数。独立两路线运行预留峰值为 8，首个开始输入到首句 137 ms，单轮图像上传峰值 2,094,080 字节，准入估算资源峰值 67,160,180 字节。这些均为单次测量，不是 P95 或真实物理内存峰值。

证据：[原生](validation/requests/requests-native.log)、[宿主](validation/requests/requests-host.log)、[lint](validation/requests/requests-lint.log)、[浏览器结果](validation/requests/browser-results.json)、[浏览器日志](validation/requests/requests-browser-run.log)、[交错账本](validation/requests/request-lifecycle-metrics.json)、[两路线测量](validation/requests/browser-metrics.json)、[SDK](validation/requests/requests-sdk.log)、[对象校验](validation/requests/requests-release.json)。GitHub 远程运行结果以 PR 检查为准。

![引擎的中文画布回归](validation/requests/dialogue.png)

## 边界

取消槽仅表示该请求的逻辑终态已确定。浏览器不能取消的音频解码仍占物理执行名额，直到实际结束。PNG 解码、字体塑形及快照序列化仍为原子工作。当前没有完整 Worker 协议、通用资源 DAG、长时间内存趋势或物理 GPU 验收；本轮重复周期不是数小时稳定性认证。
