# Web 执行域与加载屏障

核对日期：2026-10-03。Web 默认启用两个独立 Dedicated Worker；不要求共享 WASM 内存、SharedArrayBuffer 或跨源隔离。任务的 ParallelAll 仍是同一 Core 的确定性逻辑并行。

## 执行位置

| 工作 | owner |
| --- | --- |
| 输入、手势解锁、辅助 DOM、生命周期 | 主线程 Host |
| WASM Engine、Player/Core、Content/Assets、正文排版、Presentation、wgpu | 独占 Runtime Worker |
| 游戏对象获取、长度与 SHA-256 校验、PNG/WebP 解码及预乘像素转换 | 独立 Asset Worker |
| 字体加载、GPU 资源创建与分块上传、OffscreenCanvas 绘制 | Runtime Worker |
| MP3 解码 | 浏览器 decodeAudioData，主线程 Host 有界准入 |
| 已提交声音的播放、循环、设备增益包络 | 浏览器 Web Audio；控制与音频位置采样在主线程 |
| IndexedDB、文件导入导出、保存回执 | 主线程 Host 的独立异步存储作业 |
| 存档身份、版本和快照 SHA-256 校验 | Runtime Worker 的独立只读 RPC；主线程模式使用同一 WASM 导出 |

入口及启动对象先由 bootstrap 验证。主线程默认不实例化游戏 WASM；把已验证字节副本交给两个 Worker，各自实例化一个不共享的 WASM 堆。Asset 实例只调用校验、图片解码导出，不创建 Engine/GPU。主线程保留程序根中的宿主配置和对象描述，不执行剧情。

Runtime 使用自己的计时器推进逻辑与绘制，不依赖主线程 rAF 逐帧发送 tick。需要时约每 16 ms 运行一轮；静态场景停止调度，设备丢失检查另有低频唤醒。调度从空闲或暂停恢复时重置墙钟基准，等待时长不补入 Story；隐藏状态通过生命周期消息同步；页面准备、菜单及暂停仍由 Player 令牌决定是否推进 Story。计时采用所在 global 的 performance，而不是 window。

## 固定发行与启动回退

`engine.runtime_worker` / `engine.asset_worker` 是 release 中的内容摘要，SDK 身份及启动依赖闭包都包含两个脚本。旧 manifest 可省略这些字段并走主线程。Hello/Ready 核对协议版本 1、engine_build、worker 角色；Worker、glue、WASM 均来自固定发行对象。脚本先校验，再用同源不可变对象 URL 加载，与原 glue 导入采用相同发行信任边界。

Runtime 接收转移后的 OffscreenCanvas，实际创建指定后端并绘制一轮再回复 Ready。`auto` 在 Worker 内探测 WebGPU，初始化失败时替换画布并尝试 WebGL2。Worker 启动失败或能力缺失时，默认重建画布，在主线程实例化同一 WASM/Engine；Asset 启动失败则使用原有宿主资源路径。运行中 Worker 崩溃为显式错误并关闭两端，提供重新加载，不自动重放可能已经产生存储副作用的剧情。

诊断开关：`?diagnostics=1` 查看 execution；`worker=required` 要求实际 Worker 启动成功，`worker=main` 强制运行时回退，`assets=main` 强制资源回退，`backend=webgpu|webgl2|auto` 选择 GPU 后端。测试模式 `test=1` 才提供完整状态与 Worker 调试入口；生产每轮只发送紧凑宿主状态、辅助语义和命令，不复制完整 VM/存档。

## 消息、容量与取消

- RPC 使用协议、build、唯一递增 id；普通请求最多 248 项，总容量 256，保留 8 个控制名额。超时、异常或终止拒绝全部未完成调用。
- Runtime inbox 最多 256 项，每批最多 128 次白名单 Engine 调用。输入、资源及宿主回执仍执行 Core 原有的 interaction/session/request/device/locale 代次检查；额外 UI 查询核对 session 与 interaction。陈旧查询返回空结果。
- Runtime 最多保留一个未确认的状态推送。主线程确认后才发送下一份；状态合并，关键输出命令不丢弃，积压超过 248 项显式报错。延迟的 GPU 回复仍交付它拥有的命令，较旧视图不能覆盖新状态。
- 指针串行队列最多 64 项，相邻移动按 pointer 合并，按下/松开保持顺序；生命周期取消和视口变化会作废正在等待查询的结果，并取消共享手势。旋转前未结束的按下不能在新布局里选择另一个控件，下一次新手势正常处理。边沿事件超限显式报告容量故障。
- 主线程在 Runtime resize 同步完成后，从最新 owner snapshot 发布辅助控件和恢复提示布局。不能使用 `RemoteEngine.draw` 排队 resize 时返回的旧视图，否则画布及命中区域已更新而 DOM 仍保留旧坐标。回归按本次 resize 的请求身份及宽高核对，反复转回同一尺寸不能复用历史回执。
- Asset inbox（排队与执行合计）最多 64 项，最多 4 项执行。Host 沿用资源池 4、解码池 2、上传池 1 和内容字节预算。取消获取使用 AbortController；取消的不可中断解码占据 RPC/作业名额直到物理完成。
- 共享获取按消费者计数。编码数据缓存仍在 Host，图片解码用转移副本；解码结果以 ArrayBuffer 从 Asset 转交 Runtime，主线程不复制整幅 RGBA。GPU 上传沿用每轮 2 MiB 行预算。内容分包交付使用 structured clone，避免拆走共享消费者仍持有的数据。
- IndexedDB 保存拥有独立终态槽，跨剧情会话收尾；读档、导入和资源候选仍检查修订/会话。销毁关闭 Worker、声音、事务入口、监听器及请求槽。
- 存档检查不调用 Engine pump、绘制或时钟推进；每个坏槽独立返回错误，健康槽继续列出。保存校验可等待 Worker，但写事务内只做完整记录比较和写入，避免异步等待导致事务提前结束，也拒绝校验期间同版本号记录被替换。
- 发行存档历史只持有主键、日期与版本元信息，最多两份快照读取/检查在途；导出按点击读取，不提前缓存所有 JSON。关闭窗口取消余下检查，已发出的 RPC 完成账本但不改旧界面或触发迟到导航。其它标签页替换记录后，导出与导航使用最新的读取/检查结果。

WebGL2 丢失时重建并转移新画布，WebGPU 保留 Runtime 所持表面。新设备只恢复 GPU/资源，不重放剧情，恢复后等待明确继续。Runtime 在发现丢失或接收恢复指令后停止对旧设备绘制。

## 加载不控制正在播放的音频

PauseToken 分别计数 Story 逻辑暂停与音频暂停。

| 原因 | Story 逻辑 | 已启动的 Story 音频 |
| --- | --- | --- |
| 图片、音频、字体、内容或语言资源屏障 | 等待候选准备 | 继续播放 |
| 菜单、设置、历史、存档 | 暂停 | BGM 继续，Voice/Sfx 暂停；作者菜单音乐替换时暂停 BGM |
| 后台、外部显式暂停、读档后暂停 | 暂停 | 暂停 |
| 阻塞故障、设备恢复、队列溢出 | 暂停 | 暂停 |
| 作者显式停止、作用域结束、会话退出 | 按任务语义 | 停止对应声音 |

内部 prepare/content/locale 只影响 Story；菜单逻辑暂停与每总线暂停分开，设备/后台域暂停优先于总线释放。外部令牌即使命名相同也保持显式暂停。下一页加载保留旧画面、活动声音及资源引用，不重建 source、不重新下载 BGM、不把设备时间补进剧情。音频就绪只依赖校验/解码，不等待 resume Promise；用户手势独立解除输出锁。候选语音仍在页面原子提交后启动。

Host 按音频域、session、task 控制声音，忽略陈旧会话的启动；完成与位置观测由 Player 校验。已提交 AudioBufferSource 的循环与设备包络不需要 Worker 每帧返回 PCM；当前无需另加 AudioWorklet。主线程阻塞期间已启动声音继续由设备播放，新的音频控制、解锁、DOM 和存储回执仍要等待主线程，因此不能保证这些操作在主线程长任务期间即时发生。

## 验证与边界

`tests/host/worker-bridge.test.js` 覆盖控制预留、取消后的物理占位、协议身份、终止、指针洪泛及取消。`tests/browser/workers.spec.js` 检查两种 GPU 的实际 Worker、主线程零 WASM 实例化、900 ms 主线程阻塞期间独立时钟、有界输出、存读档、启动回退、GPU 恢复、崩溃和销毁。`tests/browser/audio-loading.spec.js` 阻塞下一页资源，验证同一循环 source/设备时钟持续前进、Story 冻结、显式菜单暂停以及资源解除后提交。通用浏览器套件继续覆盖剧情结果、语言、迟到存储和资源取消。

计时映射 Worker performance.timeOrigin 到导航时间轴；诊断单独记录 Asset 堆、实际 Runtime GPU adapter 和最多 4096 条摘要标识的资源时间，性能测试合并主线程与 Worker 的网络记录，并标记缺失/覆盖；Rust CPU 阶段与主线程宿主阶段来自不同执行域，区间仍是 inclusive/non-additive，不能求和当作主线程阻塞或 GPU 时间。PNG 解码、字体塑形等在各自 Worker 内仍是原子工作。迁移没有实现通用资源 DAG 或完整物理内存测量；两个独立 WASM 实例会增加内存。Android/Cromite 真机、Safari/iOS 和物理 GPU 性能需分别验收。
