# 结构化诊断与阶段追踪

本阶段面向引擎、SDK 与作者工具。测试工程用于触发故障和测量，不是产品主体。原有发行标签与下载包不变；使用本阶段能力需要从对应源码构建配套 SDK。

## 作者诊断

```sh
./dist/novelc -p examples/rain-letters check
./dist/novelc -p examples/rain-letters --diagnostics json check
```

失败仍返回非零退出码。文本模式显示错误码、逻辑位置、可获得的文件/行/列、JSON Pointer、引用与修复提示；JSON 模式在 stderr 输出 `{"format":1,"diagnostic":...}`。正常进度仍在 stdout。SDK 生成 `diagnostic.schema.json`。

JSON 语法及可定位的 Schema 错误、TOML 解析错误保留解析器报告的位置。fragment 的场景、Cue、函数、块、操作建立作者侧来源索引；资源/场景等引用可定位到字段值。场景和 Cue 可以同名，索引按声明类别区分。列号为从 1 开始的 UTF-8 字节列，不是屏幕字符宽度。来源索引不进入 Program、存档或运行发行对象，不影响逻辑 revision。

尚未为所有旧的 CLI 字符串错误补齐来源位置：文件系统/锁/媒体检查等会获得统一错误封套，但可能只有 `project` 位置；翻译契约的跨文件详细来源链也尚未全面覆盖。没有位置时不编造行号。这不是完整 LSP 或运行时 source map。

## 运行时错误

Diagnostic 保留原有 code/location/message，并可带 details：错误域（content/core/prepare/render/storage/host）、操作、阶段、请求、会话、设备、任务、引用与恢复选项。旧的三字段 Diagnostic 仍可读取。原始原因保留在内部；正常界面的提示由 Fluent 简中/英文消息生成，不直接显示浏览器异常内容。准备错误提供重试，其他故障不会一律显示无效的重试按钮。

资源获取/校验、音频解码、图像解码与上传、准备提交、存储事务和核心故障保留各自边界。保存错误使用发起保存的会话身份，即使回执迟于会话替换。IndexedDB 写入回调抛出的异常会中止事务并报告终态，防止配额错误使完成槽悬挂。准备失败与存储失败继续保留当前场景/会话；迟到失败不能改写已有成功结果。

模板字体补齐新增中文提示的字形；原生测试检查随 SDK 提供的字体覆盖内嵌 UI 消息。

## 诊断导出

以 `?diagnostics=1` 打开正式播放器，显式启用本地记录。在浏览器控制台执行：

```js
window.nirDiagnostics.snapshot()
window.nirDiagnostics.download()
```

下载的 `nir-diagnostics.json` 固定关联当前 release 和实际 WASM 对象。仅启用诊断不开放剧情操作或故障注入接口；这些仍限于 `?test=1`。测试模式默认记录，`?test=1&trace=0` 可以关闭记录，用于行为对照。

记录器采用最多 4096 条的环形存储，超出后覆盖旧条目并报告 dropped。固定字段白名单只保留阶段、稳定身份、代次、错误码、字节数和时间；不复制完整异常 message/cause、对白、动作参数、玩家名字、变量、快照或 URL。无自动上传。稳定内容 ID 本身可能具有业务含义，作者不应把玩家个人数据用作 ID。

请求槽在预留时冻结来源代次；取消、完成和迟到回调使用同一 host_request。它与剧情资源 request 分开。关联链覆盖：输入接收/派发、准备请求、资源排队/准入、获取及 hash 校验、音频解码、图像解码与纹理分配、分块上传入队、可供后续有序使用、文字准备、租约、提交、渲染提交、存储事务、设备恢复。准备请求带 Cue 与逻辑位置；资源阶段带对象 hash。

所有微秒时间为十进制字符串，使用页面 performance 时钟。Rust 原子解码/分配、上传和呈现准备给出 start_us/end_us；播放器逻辑事件的 at_us 是宿主消费命令时的观测时间。事件按记录顺序导出，跨层延迟可能使原子阶段的 start_us 早于前一条记录；分析应使用各阶段实际字段。上传入队与有序可用不表示 GPU 已完成，render_submitted 也不代表像素实际到达屏幕。GPU 时间和物理进程内存标为未测量。初始化在宿主启动前失败时仍由启动错误壳报告，不会产生完整宿主追踪。

## 重复测量

先构建正式 SDK 与静态目录，再使用与浏览器回归一致的 Chromium/显示后端配置：

```sh
bun run test:performance
```

默认分别在 loopback 和固定网络条件下运行 20 次独立浏览器上下文启动、20 次同上下文暖缓存重载，以及一组 30 次保存恢复/返回标题循环，其中 3 次真实 GPUDevice 销毁。固定网络使用 Chromium CDP 设置 40 ms 延迟、20 Mbps 下载、5 Mbps 上传；吞吐量采用十进制 Mbps 转换为 bytes/s，并非真实公网模拟。可用 `NIR_PERF_SAMPLES`、`NIR_PERF_CYCLES` 调整样本数；少量试跑不能替代默认基线。测试数据仅写入 `reports/`。

`performance-baseline.json` 保留每次样本、浏览器、实际适配器、发行/引擎身份，并汇总导航到首句提交、已准备场景上打开菜单输入到提交的中位数/P95，报告活跃时段 rAF 间隔分布。`performance-recovery.json` 保留循环后的 WASM 线性内存容量、资源准入估算与活动请求数。动画采样明确测 rAF 调度间隔，不等于 GPU 执行时长。导航时间包括测试驱动在标题就绪后发起开始操作的延迟。

独立 context 隔离 HTTP 缓存，但共用浏览器/GPU 进程，不能冒充全冷进程启动；暖缓存报告零传输字节的对象数以核对实际命中。所有测量启用诊断。固定样本数的结果用于环境内比较，不作为所有硬件的性能承诺。30 次循环不是数小时长稳认证；WASM 容量增长也不直接证明泄漏。

常规 PR CI 运行功能回归及规模冒烟门禁；主干 push 或手动启动工作流额外运行完整性能测量并保留产物。暂不为软件渲染环境设硬件性能通过阈值。

## M2.3 多模块基准

`tests/performance/modules.spec.js` 在两个网络配置下分别测量 3 章和 32 章作品，每个配置进行 5 组预取开关配对，组间交替先后顺序。主流程在章前等待 1 秒，再直接调用目标章；章节返回后继续下一章，最终重入第一章。章前等待使现有 CFG 预取能够预测目标，而已返回章节可以被驱逐。测试不会为了等预取完成而延长阅读时间。

另有一次 32 章容量配对，用合法 JSON 空白扩充代码对象至每章 640 KiB，累计编码内容超过 16 MiB；重新计算发行引用与摘要并独立校验。这是**合成容量检查**，不代表作品解析性能，也不与语义场景耗时混合。检查第一章发生逻辑重新加载，HTTP 缓存命中不被误认为内容未驱逐。

每次路线后保存、回退、返回标题、恢复，再检查清理。门禁检查剧情变量／对白一致、编码驻留和暂存分别不超预算、请求及暂存最终归零、不加载非当前语言正文、无异常请求失败。逻辑对象请求和浏览器资源传输分别统计。

`moduleCount` 统计章节数，另有一个独立主流程模块；每章一段简短对白，复用媒体。这是可重复的内容加载场景，不代表复杂演出或长篇排版的全部成本。预取开关的源码 revision 不同，配对时仅排除这个 revision 和预取配置字段核对剧情身份，并再次比较实际对白／变量路线。固定 1 秒阅读停顿共约 12 分 20 秒，加上启动、执行及构建，完整套件通常需要数十分钟。

`performance-modules-*.json` 为独立的 format 1 报告，包含场景、编译准备耗时、发行／引擎摘要、实际适配器、原始章节样本、分段事件和状态采样，以及 median/P95。章节时间从触发输入到目标对白对应的渲染提交，包含预取命中的章节。事件按增量位置收集；未收集到的环形日志区间使测量失败，不补零。活跃 rAF 间隔仍只是调度间隔；内容编码字节、宿主暂存、媒体准入估算、WASM 线性容量分别列出。

```sh
# 少样本、3 章语义路线和完整容量检查；不能当作性能基线
bun run test:performance:smoke

# 完整软件渲染基准：沿用 CI 的 CHROMIUM / NIR_CHROME_ARGS / Xvfb 环境
bun run test:performance

# 本机真实 GPU；CHROMIUM 指定浏览器，必要时 XVFB_RUN 指定 xvfb-run
bun run test:performance:hardware
```

硬件入口记录 CPU、内存、系统及可获得的驱动信息。测试在导航前拦截引擎实际的 `requestAdapter` 调用，验证其身份及 fallback 状态；不使用另一个独立探测的适配器替代引擎证据。软件适配器、身份缺失或引擎无法启动均不能生成硬件通过结果。NixOS 可在 `nix develop` 内执行；本机验证使用 NVIDIA 原生 Vulkan 与 Xvfb，无需修改系统配置。

软件完整／冒烟入口需要与 CI 相同的 `CHROMIUM`、`NIR_CHROME_ARGS` 环境；无显示服务时可用 `XVFB_RUN` 指定工具位置。先构建 SDK 和示例发行包，再运行测量。`NIR_PERF_SCALE_REPETITIONS` 可减少语义路线的重复次数；不足 5 次时报告标记为 smoke。容量场景仅配对一次，作为确定性检查。

默认继承 Playwright 的 `retain-on-failure` 追踪，执行过程中会采集截图／快照，成功后才丢弃；它与运行时诊断都带来观测开销。建立耗时硬门槛前应固定这项配置并评估其影响。可用 `--trace=off` 建立另一套基线，不能与默认追踪开启的结果直接混为一组。

硬件报告对照首句 P95 ≤ 4 秒、已准备场景菜单输入到提交 P95 ≤ 50 ms 的设计建议，当前仅报告是否满足，不把首次测量直接设为耗时门禁。后续阈值调整应固定设备、浏览器和场景，并保留基线证据。合成容量与少样本报告不得用于这类验收。

本阶段实测证据见 [诊断与性能验证](validation/diagnostics/README.md)。

## 启动与章节优化对照

性能对照固定关闭 Playwright trace，运行时诊断仍开启。基线与优化版使用相同设备、浏览器、网络和样本数，分别保存 `reports/` 产物，不能混用 M2.3 开启 trace 的历史耗时：

```sh
bun run test:performance:hardware --trace=off --grep 'navigation baseline|module scale shaped-32'
```

启动报告保留阶段事件及对象 ResourceTiming；章节报告增加 fetch、DNS、connect、request、response 各时间点。`module_requested → module_fetch_started` 表示资源池排队；`object_downloaded.start_us/end_us` 包括 fetch 到完整读取字节；`object_verified.start_us/end_us` 单独记录长度与摘要校验。ResourceTiming 的 `requestStart → responseStart` 和 `responseStart → responseEnd` 分别帮助区分首字节等待与下载，不将浏览器/CDP 的等待直接归因为服务器。

`encodedBodySize` 是 HTTP 内容编码后的正文长度，`decodedBodySize` 是解压后长度；两者与内容账本的“对象编码字节”不是同一层概念。预压缩只减少网络传输，不降低驻留预算。长尾记录需包含触发章节、预取状态、请求阶段与实际适配器，不能仅凭一次最大值断言根因。

`host_work` 补充资源池活动／排队数、共享下载、内容与媒体任务、owner 回调／请求槽、音频状态，以及最多 128 条待处理内容任务身份和阶段，不包含正文。性能路线失败时在关闭上下文前导出 `performance-failure-*.json`，保留这些状态、阶段事件和资源时间线。

## 交互与主线程分段计时

诊断开启时，`diagnostics().performance` 独立保留累计阶段统计与最近 64 个 owner turn，超过容量只覆盖旧明细，不挤占请求事件环。`turns[].stages` 的 `start_us`、`end_us`、`duration_us` 为浏览器单调时钟的整数微秒数；这与原有事件使用十进制字符串的格式不同。`?test=1&trace=0` 关闭详细计时，完整测试状态接口仍可使用。

阶段包括宿主 `wake_wait`、`event_handling`、`semantics`，Rust `compact_state`、`projection`、`draw`，以及渲染器 `prepare.layout`、`prepare.glyphs`、`vertex.build_write`、`surface.acquire`、`command.encode`、`queue.submit`、`present`。图片上传另分为 `image.convert` 和 `image.write`。`draw` 表示画面比较与替换；`semantics` 分别记录 Rust 语义序列化及 DOM 更新。事件处理可包含投影、资源准备和上传，因此 `stage_semantics` 明确为 `inclusive_non_additive`：各阶段累计耗时不能直接相加作为整轮耗时。`wake_wait` 发生在 turn 开始前；由输入产生的状态读取也可能早于它所属的记录轮次。

所有 GPU API 阶段均测 CPU 调用耗时，不等待 GPU 完成。文字缓存统计反映命中、未命中、LRU 淘汰及当前项数；128 项是缓存条目数的收敛目标，单次布局的完整工作集超过该值时仍受保护，不是物理内存上限。

`Engine.host_state()` 是同一 SDK 内部的精简宿主接口，包含调度、输入令牌、滚动和计数，`has_dialogue` 只表示对白存在。完整 `Engine.state()`／`window.__nir.state()` 保持兼容。宿主在引擎发生状态变更后使快照失效，不缓存跨变更的交互令牌。

```sh
bun run test:performance:hardware --trace=off --grep 'navigation baseline|module scale (shaped-32|capacity-32)|prepared interactions'
```

新增交互场景先经过 32 章，积累 66 条历史；章节对白为固定 40 行，再分别测量菜单打开／关闭、历史滚动／翻页及存档恢复各 30 次。场景使用真实输入路径浏览溢出正文，不绕过阅读语义。`performance-interactions.json` 保留原始样本、发行身份、实际适配器、阶段明细与恢复后的剧情一致性检查；没有新诊断接口的旧 SDK 仍能生成同一工作负载的耗时基线。结果见 [交互优化验证](validation/interaction/README.md)。

仅在排查获取长尾时启用额外 CDP 网络事件和浏览器 trace：

```sh
NIR_PERF_NETWORK_TRACE=1 bun run test:performance:hardware --trace=off --grep 'prepared interactions'
NIR_PERF_NETWORK_TRACE=1 NIR_PERF_DISABLE_HTTP_CACHE=1 bun run test:performance:hardware --trace=off --grep 'prepared interactions'
```

网络事件最多保留 20,000 条，报告包含截断标记；浏览器 trace 输出为 `reports/performance-interactions-trace.json.gz`。禁用缓存开关仅在网络诊断开启时生效，只作用于该测试上下文。额外追踪会改变观测开销，应单独归档，不混入正式前后基线。CDP 缓存标记、字节数及阶段时间是定位线索，不能单独证明浏览器或服务器根因。
