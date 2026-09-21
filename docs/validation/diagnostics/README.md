# 诊断与重复测量验证

测试日期：2026-09-21。最终静态发行 `26ac1a3f351e9cac61b317c0bc0e87dcdeb29de9fd2220339c968cd6628afdf2`，12 个对象，共 6,803,481 字节；WASM 为 3,942,144 字节。SDK 身份 `cc5cff64caa0c1fa4d011acf94e70c93b3511fd01cab01e405bad5dc65b307f5`。

环境：Linux、Chromium 153.0.8010.52、X11/Xvfb 1920×1080，页面 1280×800。浏览器以匹配的 SwiftShader/Vulkan 参数运行；同为 low-power 的浏览器适配器探测返回 vendor=google、architecture=swiftshader、fallback=true。wgpu 的 Web 后端只报告 BrowserWebGpu/Other，名称为空。未验证物理 GPU。开发者绝对路径已替换为 `/nir`、`/build-home`。

| 检查 | 结果 |
|---|---|
| 原生与呈现测试 | 61 项通过（xtask 60，呈现 1） |
| 宿主测试 | 18 项通过，含固定种子 20,000 次请求交错 |
| 最终目录浏览器回归 | 18 项通过，0 失败、跳过、flaky；102.9 秒 |
| 性能/恢复用例 | 2 项通过，0 失败、跳过、flaky；162.4 秒 |
| fmt、原生/WASM lint、依赖边界 | 通过；vendor/wgpu 原有两项 unused-import 警告保持原状 |
| 独立 SDK 与 JSON 作者诊断 | 通过；不依赖 Cargo，能定位到具体资源引用字段 |
| 重复构建、锁漂移、对象校验 | 通过 |

错误注入覆盖资源获取、音频解码和 IndexedDB 写入配额失败，验证失败后重试及当前会话保留。作者测试覆盖同名场景/Cue 的引用位置、解析器行列和 UI 字形。浏览器对照追踪开/关后的逻辑 trace、变量终态与结局；导出不包含注入的私密原因字符串。设备丢失使用真实 GPUDevice.destroy；正常 GPU 提交不代表硬件或声音实际输出认证。

## 重复测量结果

每种启动方式 20 次；loopback HTTP、未限速、诊断开启。独立 context 隔离 HTTP 缓存，但浏览器/GPU 进程共享；暖重载每次都有 9 个对象为零传输字节。导航指标含标题就绪后自动按开始的测试驱动延迟；“输入”专指已准备场景上打开菜单，不能据此声称所有剧情输入均满足同一延迟。

| 指标 | 独立缓存上下文 | 暖缓存重载 |
|---|---:|---:|
| 导航到首句提交中位数 | 1707.4 ms | 1449.5 ms |
| 导航到首句提交 P95 | 1987.1 ms | 1611.0 ms |
| 菜单输入到提交 P95 | 17.1 ms | 16.8 ms |
| 活跃 rAF 间隔 P95 | 183.3 ms | 200.0 ms |
| 活跃 rAF 间隔 >33.3ms 比例 | 74.46% | 72.41% |

活跃调度间隔明显未达到设计的 60Hz 演出参考目标。软件 WebGPU 与浏览器合成环境下的这个结果应保留为失败的性能指标，不因功能用例通过而标为帧性能达标；它也不能单独归因于引擎或代表物理 GPU。GPU timestamp、真实呈现延迟和物理进程内存未测量。

30 次恢复/返回标题循环包含 3 次设备销毁。每轮末活动请求为 0；WASM 容量从 9,306,112 到 9,306,112 字节，资源准入估算从 57,362,055 到 57,362,055 字节。它们分别是线性内存容量与预算账本值，不是 JS/GPU/进程物理内存。此短周期运行不等于数小时无泄漏认证。

## 证据

[原生测试](diagnostics-native.log)、[呈现测试](diagnostics-presentation.log)、[原生 lint](diagnostics-lint.log)、[WASM lint](diagnostics-wasm-lint.log)、[宿主测试](diagnostics-host.log)、[独立 SDK](diagnostics-standalone.log)、[发行校验](diagnostics-release.json)、[浏览器日志](diagnostics-browser.log)、[浏览器结果](browser-results.json)、[脱敏阶段追踪](diagnostic-trace.json)、[性能日志](diagnostics-performance.log)、[性能测试结果](performance-results.json)、[逐次启动测量](performance-baseline.json)、[逐轮恢复测量](performance-recovery.json)。

这些是本地最终目录的实测证据。GitHub 远程检查以 PR 的工作流状态为准；使用和未覆盖边界见 [诊断说明](../../DIAGNOSTICS.md)。
