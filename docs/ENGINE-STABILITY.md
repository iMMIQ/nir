# 引擎稳定性：有界调度与请求生命周期

本轮继续完善 NIR 引擎底层协议。`examples/rain-letters/` 仅作为端到端测试工程，已发布的 v0.1.0 标签及下载包保持原身份。此前六份设计的对照基线保存在 [差距报告](DESIGN-GAPS.md)。

## 已实现

- 输入、资源、音频和存储完成进入统一宿主 inbox，由 owner 任务消费；宿主与 Player 队列都有容量限制。输入最多占到 128 项准入水位，总容量 256 项，溢出明确报错。正在处理的批次也计入容量，回调产生的新事件留到后续轮次。
- 每轮共享 10,000 单位语义预算，覆盖事件、指令与时钟边界。移除每个事件和每次时间推进重新领取预算的路径；未处理事件和时间余量可继续执行。有效选择优先于同轮超时，旧会话时间不能进入新会话。
- 独立 PauseToken 支持同原因的多个持有人；释放一个令牌不会提前恢复 Story 或音频，令牌可以安全地晚于 Player 销毁。
- 准备替换、失败、返回标题和设备丢失发送明确取消；共享获取由最后一个消费者触发中止。失败准备不能被迟到成功提交，重复保存失败不能覆盖已完成的成功回执。
- 获取、音频解码和资源交付共用四个全局执行名额。不能中止的音频解码在实际结束前继续占用名额，等待中的取消项立即离队。资源生产者等待 owner 消费，避免完成字节无限堆积。
- 图像按完整行分块进行颜色预乘和 GPU 上传，每轮最多 2 MiB；未完成纹理不可绘制，取消与设备重建会释放。大于单轮上限的测试背景实际跨多个轮次上传。
- 新增 GitHub Actions 工作流，覆盖格式、lint、依赖边界、原生测试、宿主队列测试、WASM/SDK 构建、独立 SDK 与 Chromium 浏览器回归。SDK 构建使用替换文件的方式更新 CLI，预览服务器运行时也能完成构建。

具体协议及边界见 [ADR-007](ARCHITECTURE.md#adr-007有界-owner-调度与分块上传)。

## 验证

测试日期：2026-09-21。最终静态构建身份：`de8bcd0f393f11f18ad4dbf6cff43ab0d2b4876b5a703e77526fca9c92a0c132`；12 个对象，共 6,729,037 字节。

| 检查 | 结果 |
|---|---|
| 原生测试 | 55 项通过；额外复测低预算下选择优先于超时 |
| 宿主调度测试 | 9 项通过，覆盖容量、重入产出、资源分片、恢复、取消与共享获取 |
| 真实浏览器 | 12 项通过，0 失败、0 跳过、0 flaky；125.1 秒 |
| 格式、lint、依赖边界 | 通过；第三方 wgpu 已有 unused-import 警告保持原状 |
| WASM / SDK | release 构建、对象校验、独立 SDK 与锁漂移检查通过 |
| 隐私复查 | 公开源文件与脱敏测试记录扫描通过 |

浏览器为有窗口的 Chromium 153.0.8010.52，实际适配器为 SwiftShader 软件 WebGPU。检查了完整画布像素和截图，两条路线、刷新读档、语言实例边界、回退、真实设备丢失、后台恢复、损坏对象、子路径、触摸输入均通过。新增检查验证输入先入队、40 项输入批次跨轮消费，以及退出准备后真实网络请求中止、重新准备成功。

单次两路线运行记录：单轮图像上传峰值 2,094,080 字节（上限 2,097,152），共 8 个图像上传分块；首个开始输入到首句 338.9 ms，GPU 提交 83 次，资源准入估算峰值 67,160,180 字节。测试同时断言静态标题停止提交、宿主队列不超过 256 项、每轮语义工作不超过 10,000。以上是单次开发环境测量，不是性能分位数或硬件成绩。

上表保留初次本地验证。PR #1 合并前的 [GitHub CI](https://github.com/iMMIQ/nir/actions/runs/35557877308) 在提交 `38430ab3105c94f06ee0175393dca68eec72be2f` 上通过原生检查、SDK 构建及 13 项浏览器测试；[浏览器结果](validation/stability/ci-browser-results.json) 已归档。后续修订加入每轮开始时的设备检查，并在 CI 显式使用 X11/Xvfb 和匹配的 SwiftShader/Vulkan 合成配置。未验证物理 GPU、移动真机及长时间压力场景。

证据：[原生测试](validation/stability/stability-all-native.log)、[低预算选择](validation/stability/stability-deadline.log)、[宿主测试](validation/stability/stability-host.log)、[浏览器结果](validation/stability/browser-results.json)、[运行日志](validation/stability/stability-browser-run.log)、[测量](validation/stability/browser-metrics.json)、[独立 SDK](validation/stability/stability-sdk-verification.json)、[发行校验](validation/stability/stability-release.json)。

![分块上传后的中文测试画面](validation/stability/dialogue.png)

## 仍有边界

PNG 解码、字体加载与塑形、快照序列化和单条语义操作仍不能在中途让出。宿主的 4 ms 检查发生在原子工作之间，不能宣称每个浏览器任务都在 4 ms 内完成。2 MiB 限制针对场景图像上传，不涵盖 glyphon 内部字形图集上传。

上述记录对应第一阶段。后续逐请求终态预留及取消交错验证见 [请求生命周期进展](REQUEST-LIFECYCLE.md)。资源调度仍采用有限阶段，并非完整需求 DAG。仍需补充 Worker、完整分层缓存、物理内存回报、长稳压力测试和真实硬件性能验收。
