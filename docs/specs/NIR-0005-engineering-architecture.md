# NIR-0005：Rust / WASM / wgpu Web 首版工程架构

状态：设计提案 v0.1；日期：2026-09-20。

范围：承接 NIR-0001 至 NIR-0004。第一版只做 Web 播放器；剧情、场景、文字、游戏 UI、演出调度保留 Rust/WASM，GPU 使用 wgpu。已有内容直接遵循 NIR；不包含旧引擎导入器、作者编辑器、原生发行端、在线账号或云存档服务。

本文件是一份工程规范，不是已有引擎的代码或性能报告。附带 Python 检查器只验证给定 Cargo 元数据图中的包级依赖规则。示例元数据完全由脚本构造，未执行 Cargo、Rust/WASM 编译、GPU 或浏览器测试。

## 1. 结论

采用模块化单体、单一逻辑写入者、显式应用编排和平台适配。多个模块不意味着多个进程，也不需要微服务；一次 Web 发行仍是静态入口、配套 WASM/JS/Worker 和版本化内容对象。

工程设计的核心不是“有多少 manager”，而是：谁拥有状态、谁允许修改、异步结果何时可提交、失败如何收敛、依赖如何自动检查。

默认不引入通用 ECS、全局事件总线、服务定位器、运行时动态插件、通用 DI 容器或跨平台线程抽象。容器可以用 std，不以 no_std 作为“纯核心”的前提。平台无关意味着无浏览器、GPU、隐式墙上时间和隐式 I/O 依赖。

## 2. 前序设计如何落地

| 前序契约 | 工程实现位置 |
|---|---|
| Program / Executable / Snapshot / DeviceCache 分离 | format、core、presentation、renderer |
| 单剧情流＋可序列化任务 | core::vm / tasks / scene |
| PendingActivation 不重复执行 | core::pending、player::prepare |
| 联合预算、ReadyLease | assets::budget、player::prepare、renderer 的实际驻留记录 |
| 独立语言与安全切换 | content::locale、player::locale_switch、presentation |
| 固定发行对象图 | content::release、player::session_release |
| ResumeMap / SemanticCostMap | format::executable、core::restore、compiler |
| 编译工具不进入播放器 | 单独依赖根 novelc / player-web |

这些是同一条播放流水线的组成部分，不是四个分别实现状态与存档的子引擎。

## 3. 仓库与 crate 边界

建议用一个 Cargo workspace。共享 lockfile、版本约束、lints 和 profile；虚拟 workspace 显式设置与固定工具链匹配的 resolver。[R1]

```text
nir/
  Cargo.toml
  Cargo.lock
  rust-toolchain.toml
  .cargo/config.toml
  crates/
    nir-format/
    nir-core/
    nir-content/
    nir-assets/
    nir-presentation/
    nir-player/
    nir-render-wgpu/
    nir-platform-web/
    nir-compiler/
  apps/player-web/
    src/                  # Rust 组装、适配器 newtype、生命周期
    host/                 # 轻量 JS/TS、Worker 入口、辅助语义、错误壳
  tools/novelc/
  xtask/
  fixtures/               # 小型、自有或授权的测试内容
  tests/browser/          # 端到端测试项目，配置指定真实后端
  benchcases/             # 稳定内容、输入记录、测量政策
  docs/adr/
  docs/specs/
  dist/                   # 生成物，不作为源码
```

第一版 9 个库、3 个入口足够。core 内 vm/tasks/scene/save，presentation 内 text/ui/layout，player 内 prepare/session/save/locale，不先拆成几十个独立 crate。独立 crate 的理由是依赖边界、平台差异或独立构建，而不是某个结构体名字以 Manager 结尾。

| 库 | 负责 | 不负责 |
|---|---|---|
| nir-format | 稳定 ID、版本化 NIR/执行/存档/清单 DTO、文本/UI 数据契约 | 会话对象、GPU、浏览器、全局服务 |
| nir-core | 验证后的执行表示、VM、变量、任务、逻辑场景、快照/恢复 | fetch、UI 菜单、GPU、IndexedDB、异步执行器 |
| nir-content | release 图、对象身份、模块/语言解析与契约、资源定位 | I/O 执行、解码器、图形、剧情进度 |
| nir-assets | 需求 DAG、预算预留、任务去重/调度、CPU 表示与缓存政策 | 浏览器 fetch 调用、wgpu 对象、剧情变量 |
| nir-presentation | 文字塑形/布局、游戏 UI 状态/布局、绘制/语义数据 | VM 写入、浏览器节点、wgpu 对象 |
| nir-player | 会话编排、输入路由、准备、语言/读档事务、暂停、存储意图 | GPU API、Web API、VM 算术/分支细节 |
| nir-render-wgpu | wgpu 设备/surface、纹理、GPU 字形图集、pass、管线、绘制 | VM、语言选择、下载、存档 |
| nir-platform-web | Web 的 I/O、音频、存储、输入、Worker/DOM 适配原语 | 分支、自动阅读规则、资源重要性决策 |
| nir-compiler | NIR 检查/优化、准备配方、恢复表、资源构建 | 运行中 Player、浏览器、发行页交互 |

nir-format 是小型共享契约，不是名为 common 的杂物箱。CoreIntent 属于 core，AppEvent/HostCommand 属于 player，DrawPacket 属于 presentation，浏览器对象属于 platform-web。

## 4. 编译依赖图

箭头表示“依赖”，不是调用顺序。

```text
nir-core             -> nir-format
nir-content          -> nir-format
nir-assets           -> nir-format
nir-presentation     -> nir-format
nir-player           -> nir-format/core/content/assets/presentation
nir-render-wgpu      -> nir-format/presentation
nir-platform-web     -> nir-format
nir-compiler         -> nir-format/core/content

player-web           -> 播放器各库（不包含 compiler）
novelc               -> nir-compiler/content/format
xtask                -> 调度外部工具进程（不链接播放器各库）
```

player 是应用政策层，不依赖具体 renderer/host。apps/player-web 是唯一同时认识 Player、WgpuRenderer、WebHost 的组装处；通过本地适配器/newtype 实现 ports，或匹配明确的命令枚举。具体 renderer/host 无需依赖 player 反向调用它。

所有库禁止依赖 apps/tools。编译器可以复用 core 的语义验证和参考解释，不允许 core 依赖 compiler。不要将完整优化器放进 core 的某个默认 feature。

presentation 的输入是它自己的 PresentationInput/UiModel，Player 将 core 的只读视图映射过来。renderer 因而不会通过 presentation 间接引入 VM。若采用 glyphon，依赖 wgpu 的部分只进入 renderer；CPU 排版部分独立保持。[R10]

Cargo metadata 可提供依赖节点、依赖种类与重命名后的真实包身份。[R2] 附件按这张图执行检查。它检查当前提供的解析图；未启用 feature、未纳入的 target、源代码 API 泄漏及精确链接单元需要另外检查。

## 5. 状态所有权

| 状态 | 唯一逻辑所有者 | 其他模块如何使用 |
|---|---|---|
| PC、变量、帧栈、随机、PendingActivation、Await | core::Session | 只读查询＋类型化输入 |
| 逻辑场景、任务参数、动画进度 | core | 可采样只读视图；不是 GPU 句柄 |
| 玩家生命周期、自动/快进政策、暂停原因 | player | UI 展示投影，不直接改字段 |
| 菜单、焦点、滚动、文本布局 | presentation | 输出 action/hit/semantic 状态 |
| release 与语言解析 | content 值对象；player 决定提交 | 所有模块使用固定的解析身份 |
| 资源预算、请求消费者、CPU 缓存政策 | assets | 实际分配方回报；由 coordinator 合并 |
| GPU 纹理、图集、临时面、管线 | renderer | generation-tagged 不透明键与租约 |
| AudioContext、AudioNode、IDBTransaction、AbortController | WebHost | 结果变成显式事件 |
| Profile/Preferences 的内存状态与写入任务 | player | core 只在声明边界接收冻结输入 |
| 持久化存档 | storage adapter 的提交事务 | SaveCoordinator 在事务完成后更新已提交状态 |

单一所有者针对权威状态。派生缓存可以有副本，但必须注明源 revision，不得反向成为剧情真相。core 中的对白当前状态不与 UI 再维护一份可独立推进的 say index。

Session 用 &mut 顺序更新；不可变 Program/目录可以共享引用。节点和任务使用带代次的索引，不以互相持有 Arc 的对象图表达生命周期。单个 WASM 实例内的 Rc/Arc 不自动成为跨 Worker 共享引用。

## 6. 纯同步核心与异步外壳

建议接口形状（概念接口，不是已实现 Rust API）：

```text
Core.step(input, semantic_budget) -> CoreStep
CoreStep = 意图 + 状态变化 + 下一等待 + 逻辑位置

Player.pump(app_events, work_budget) -> AppCommands
AppCommands = 资源/渲染/音频/存储请求 + 调度要求

WebHost/GpuAdapter 完成操作 -> AppEvent 入队
```

Core 不 await，不 spawn，不保存 Future。宿主请求可以用异步 Rust，但完成回调只写 inbox，不能在回调里嵌套调用 VM 或改当前场景。

主 owner 的一轮处理是：

```text
收集有界事件 -> 验证令牌与请求所有权
-> 处理生命周期/用户动作与明确时刻的事件
-> 核心推进到语义边界（遵循 fuel 和中间 marker）
-> 收集意图、推进准备
-> 形成只读呈现输入、排版和必要上传
-> 同步提交有效的 ReadyLease（无 await）
-> 编码/提交需要的帧、执行宿主命令
-> 计算下一次唤醒
```

具体同刻优先级继承 NIR，不以网络下载先后排序剧情分支。资源字节到达只是准备事实，何时激活必须由待激活状态决定。

spawn_local 在当前线程上调度 Future，并在 microtask 时机轮询；它不把大图解码自动搬到后台 CPU。[R4] 禁止 while pending 等待浏览器、block_on GPU 回调、持有整个 App 的 RefCell borrow 跨 await。阶段需要让出浏览器任务调度，不能形成无限 microtask 链占满事件循环。

## 7. 线程布局与宿主桥

目标：主线程宿主＋一个拥有 VM/呈现/wgpu 的 Runtime Worker；重解码可以放单独 Asset Worker。原型开始可将同一个 runtime 放在主线程，立即使用相同协议；完成核心路径后启用 Worker 作为目标配置。Worker 失败采用主线程同代码路径，不建立第二个播放器。

```text
Main: 输入、音频解锁/输出、DOM 辅助语义、浏览器生命周期、必要存储接口
Runtime: Player + Core + Content + Assets + Presentation + WgpuRenderer
Asset Worker: 受限解码/转码任务，返回拥有权明确的字节或图像
```

OffscreenCanvas 可以转移到 Worker，但需要在实际 wgpu/浏览器组合上做 startup probe。[R5] GPU 对象保持单 owner；不通过 unsafe impl Send 或人为 Send/Sync 绕过平台限制。wgpu 文档明确指出 WebGPU/WebGL 对象不能在线程间共享。[R3]

内部 Rust 同步调用不需要序列化。跨 Worker 才使用协议 envelope 和可转移数据；转移 ArrayBuffer 会使发送端原缓冲区失效。[R6] 不传 Rust 堆地址。浏览器快速图像路径中的 ImageBitmap/Blob 只在 app/host/renderer 桥内转移，不塞入 core DTO。

主线程从 Rust 呈现层接收有稳定 action ID 的辅助语义树，负责 DOM 映射；它不是另一套剧情菜单实现。字体、布局、游戏可视 UI 仍在 Rust/wgpu。失败恢复壳使用少量 DOM 是为了 GPU 初始化失败后仍能操作，不是替换正常播放器。

输入法 composition 与确认键区分；用户手势回调内执行需要手势的音频恢复，后续上报结果。Web Audio 的自动播放规则要求考虑用户操作，不应等待 Worker 往返以后才假定音频已解锁。[R8]

宿主协议 Hello/Ready 校验 engine build、protocol version、requested/actual capabilities；WASM、JS、Worker 按 release 配套。I64/u64 在线格式明确使用字符串、BigInt 约定或分字段，不能无约束转成 JS number。

## 8. 有界消息与任务账本

每个请求有 owner、request ID、作用域、stage、预算、取消与最终完成状态。取消最好中止外部工作，但正确性依赖晚到结果检查，不依赖 abort 一定成功。

主通道保留输入、device lost、失败与任务终态；pointer move、下载进度、普通遥测可以限频/合并。不得合并 press/release、丢失 choice 或已接受任务的唯一终态。并发任务数和终态槽预留一起限制；过载进入明确错误/暂停，不能静默丢关键事件。

无需任意组件订阅的全局事件总线。有限枚举、明确收件方和调用关系更容易追踪。

| 工作类别 | 有效性条件 | 过期结果处理 |
|---|---|---|
| 当前剧情激活/媒体里程碑 | session + activation/task + request | 不修改新会话 |
| 内容获取 | object digest + job ID；消费者各有 epoch | 经验证字节可复用，不能替旧消费者提交 |
| GPU 上传/资源 | device epoch + reservation/request | 释放旧对象，不认作新设备资源 |
| UI/布局准备 | viewport + typography + locale selection + request | 可保留公共资源，重建过期布局 |
| 存档提交 | GameId + SaveId + slot revision + snapshot digest | 独立报告已接受写入结果，不按当前 Session 丢弃 |

“所有回调检查当前 SessionEpoch”是错误的过度简化。物理资源任务可以被多消费者共享；存档事务也有独立生命周期。资源结果按内容 hash 入缓存，不意味着相关剧情请求仍然有效。

## 9. ReadyLease 与原子提交

ReadyLease 由 player::PrepareCoordinator 组装；assets::BudgetLedger 是内存准入政策的唯一所有者。renderer 持有实际 GPU 资源，presentation 持有 CPU 字形/布局；它们提供资源成本和不透明键，不各自无条件分配半套 Cue。

租约应为不可 Copy、非普通可序列化对象，不直接 Clone 成多个独立 pin。它持有保留项和代次凭据，提交后转移给具体 Scene/Task scope，失败或取消时释放。

提交前同步检查：PendingActivation 身份、操作数摘要、session/device/surface/typography、LanguageSelectionId、资源完整性与属性所有权。然后 core 捕获实际初值并提交，player 转移 pin，app 调度媒体和绘制。该短阶段不 await。

“逻辑提交成功”不是声画硬件事务。之后的物理设备失败走任务失败/恢复，不能声称从未激活。

Drop 可释放本地预算/句柄，但不能承诺执行一个异步 IndexedDB 写入或已完成 GPU 工作。需要确认的资源销毁、缓存写入使用显式命令与回执。可观测计数包括活动租约、驻留字节、未完成请求和孤立对象。

## 10. 生命周期函数与暂停

顶层生命周期建议区分 Boot、OpeningRelease、Title、Session、Restoring、RecoveringDevice、Fatal、Disposed。Session 内菜单、后台、资源准备是组合因素，不用一个超长互斥 enum 表达所有组合。

暂停用带 owner 的 PauseToken 集合，不只是 bool：菜单、页面隐藏、当前资源屏障、用户暂停、读档、设备恢复分别持有 token。同一原因也可能同时存在多个 owner。关闭菜单只释放菜单 token，不取消“页面仍然隐藏”。自动播放是否恢复按明确政策，不能靠最后一个回调设 false。

dispose 需要幂等：阻止新任务、注销监听器、取消调度、释放资源/Worker、记录或完成必要存储请求。不要假定 beforeunload 能保证完成最后一次异步保存；自动存档应在有效检查点完成。

### 10.1 保存

一致性切点复制规范快照 -> 独立 SaveJob -> 编码/验证 -> 提交存储事务 -> committed 回执。缩略图是可选附属工作，失败不破坏主存档。

同槽保存串行化，或在同一事务中使用修订检查，避免旧 SaveJob 迟到覆盖新快照。多标签页冲突不得无声后写覆盖；至少给出冲突/修订规则。

IDB 单个 put 请求成功不等于事务提交；Complete 事件对应事务成功。[R7] 提示应说明浏览器提交成功，而不是保证设备断电后的物理落盘和永久保留。

### 10.2 读档

读取/验证候选快照 -> 确认 release/恢复映射 -> 预留候选预算 -> 准备候选依赖 -> 安全边界替换 Session -> 更新令牌 -> 重建媒体/图形 -> 保持暂停。

候选尚未接受时，不能先废掉旧 Session。也不要求在移动设备上同时固定两套完整 GPU 场景；可以共享资源、保留旧逻辑快照并释放部分可重建派生资源。预算不足要拒绝或明确进入可重建流程，不能以 OOM 作为正常过渡。

### 10.3 设备重建与语言切换

设备重建只推进 DeviceEpoch，保留 CoreState、release、active locale 与 PendingActivation；从描述重建 GPU，不从章节入口重播。相关派生缓存分层失效，不清空所有已下载文件。

语言切换复用 Reserve/Prepare/Validate 的机制，但提交边界继承 NIR-0003 的对白/NVL/选项规则。读档替换 Session，设备恢复替换设备，语言切换替换呈现选择；三者不能实现为一个无差别 reload()。

## 11. 编译、验证和构建图

nir-format 只定义版本化 DTO；nir-content 负责输入限额、包与语言契约；nir-core 的受控构造过程建立可执行的 ValidatedProgram/ValidatedSnapshot。不得允许任意模块绕过验证直接构造受信 Session。

nir-compiler 使用相同语义验证，生成 Executable/ActivationRecipes/ResumeMap/SemanticCostMap。参考执行器和优化执行器在测试中对照。播放器不链接优化器，NIR 文件来自自己的 CLI 也仍视为需验证输入。

构建入口分开：

```text
cargo test -p nir-core -p nir-content -p nir-assets -p nir-player
cargo build --locked -p novelc --release
cargo build --locked -p player-web --release --target wasm32-unknown-unknown
```

这些是将来工作区的目标命令，本包没有这些 Rust crates。平台库按专门目标测试；不要以 cargo build --workspace --all-features 代替生产包命令。Cargo 多包构建可能合并 feature；需要隔离的构建分开执行。[R9]

xtask 是仓库维护入口，可调用 cargo、bindgen、优化、浏览器测试与打包，不做第二份 NIR 编译算法。novelc 是用户可重复调用的内容编译 CLI，不依赖 xtask。

依赖锁定覆盖 wgpu/glyphon/cosmic-text 的兼容组合，不逐个追 latest。使用私有小型适配模块隔离第三方 API 变化，不把所有类型都包装为一套新框架。

## 12. 错误、可观测性与安全

错误按边界分类：ContentError、CoreFault、PrepareError、RenderError、StorageError、HostError。错误包含 code、operation、稳定 ID、阶段、请求/代次、恢复选项与内部 cause；文本由 UI 本地化。底层不弹对话框；player 决定重试、保持、拒绝迁移或退出。

日志共享 release/build、SessionEpoch、Op/Cue、Task、Request、object hash、device、language selection 等关联字段。要能追踪“输入 -> 激活请求 -> 依赖 -> 解码 -> 上传 -> ReadyLease -> 提交”。渲染提交不冒充实际显示，设备就绪不冒充声音实际到达。

默认日志不包含完整对白、玩家名字、全部变量或存档。详细回放必须显式导出并提供脱敏；没有网络遥测必要时不增加上传服务。

核心/format 尽量 forbid unsafe；平台与渲染适配若确需 unsafe，局部、安全前提和测试明确，不为满足 Send/Sync 随意转换。所有输入做字节/深度/维度/任务预算限制；URI 由 release/受信 origin 政策解析；主题为受控数据，不执行任意 JS。

## 13. 测试架构

| 层 | 环境 | 验证内容 |
|---|---|---|
| 纯核心 | 宿主 Rust、虚拟时钟 | 运算、任务、Gate、快照与恢复 |
| 应用编排 | 宿主 Rust、FakeHost/FakeRenderer | 乱序、重复、取消、预算、切换 |
| 编译一致性 | 参考/优化执行器 | trace、错误、fuel、ResumeMap |
| WASM 适配 | 真实浏览器/Worker 测试 | 生命周期、序列化、IDB、对象转移 |
| 渲染 | 实际 WebGPU/WebGL2 路径 | alpha、色彩、字体、裁切、恢复 |
| 端到端 | 最终部署目录 | 读档/语言/更新/冷缓存等真实流程 |
| 长稳/故障 | 真机＋注入 | 内存、回退、重试、后台恢复 |

wasm-bindgen-test 默认运行 Node，需要明确选择浏览器/Worker 模式；仅在 Node 通过不足以验证浏览器接口。[R11] Playwright projects 可组织不同浏览器/配置，但 GPU 后端应记录真实探测结果，不能静默 fallback 后仍将 WebGPU 标绿。[R12]

FakeRenderer 验证调用和租约，不证明真实画面；原生 wgpu 离屏测试也不替代 Web。图片回归采用固定字体/输入和允许误差，不承诺跨设备逐像素一致；语义 trace 必须精确。

Fixture 建议：linear_dialogue、choice_and_stale_input、activation_save_pending、transition_restore、locale_switch_race、audio_locked、save_after_session_swap、device_loss、memory_admission、release_mix_rejected。使用小型自有素材，不把旧作品整包提交仓库。

## 14. CI 与质量门禁

每次变更：fmt/clippy、依赖规则、纯逻辑测试、内容契约检查；明确代码范围的 WASM/browser smoke。每天或合并到主干：双后端、多个浏览器、性能与内存趋势。Release：最终优化/压缩产物、托管头、资源闭包、升级/回滚、真机矩阵与配套 build hash。

依赖规则检查不能仅看直接 Cargo.toml。检查 normal/build/dev 区别、真实 package ID、间接 GPU/平台泄漏、compiler 是否进入 player 根、多份不兼容 wgpu 类型。附带工具覆盖给定 resolve 图中的这些包级问题，不处理全部 Cargo unit graph 或 features 成因。

每项关键不变量要有负责人、测试、错误码和 ADR。建议先写 ADR：core 单写、Prepare 租约、host 回调不重入、工作作用域、Worker fallback、存档独立生命周期、版本固定、编译工具隔离。

## 15. 实施顺序

| 关卡 | 最小成果 | 过关依据 |
|---|---|---|
| A 边界骨架 | workspace、IDs、core step、FakeHost、规则 | 纯核心无 GPU/Web 依赖 |
| B 真 Web 纵向样本 | fetch NIR -> Rust VM -> wgpu 背景＋中文对白＋输入 | 真 WASM 与渲染，不靠 HTML 替身；可先 main-owner |
| C 准备与恢复 | PendingActivation、预算、租约、IDB 存读 | 重复/迟到结果不污染；失败保旧 |
| D 实际演出 | 并行入场、语音、转场、回看 | 帧与加载 trace，可恢复关键路径 |
| E 国际化/发布 | 独立语言、固定 release、缓存/更新 | 语言/更新竞态测试 |
| F 优化与线程目标 | Worker 路径、增量准备、编译候选 | 与相同 core/main fallback 比较的实测结果 |

纵向样本尽早进入真实 Web，以避免连续数周只实现抽象层。无需等待所有 module 完成；沿一条链路逐步扩展。未来新增原生宿主时实现新的 root/adapter，主要更换输入、文件、音频、surface 和持久化，不提前实现原生工程。

## 16. 本次附件与验证范围

- architecture.toml：9 个库＋3 个入口的直接与传递依赖政策。
- check_arch.py：读取完整 Cargo metadata JSON，验证给定图；标准库实现，Python >= 3.11。
- examples/cargo-metadata.synthetic.json：标注 SYNTHETIC 的包图，不是真实构建。
- tests/test_architecture.py：正例和故障注入测试。
- check-results.txt：本次运行结果。

执行了 18 个 Python 测试，覆盖反向依赖、间接平台依赖、依赖别名、dev/build 区分、缺 resolve、未知 workspace、循环、多版 wgpu 以及 glyphon 间接泄漏等。未执行 Rust 编译、WASM、GPU 或浏览器验证；不能据此宣称引擎架构已经在运行中证明正确。

## 参考资料

以下官方资料核对日期：2026-09-20。库具体版本由工程选定兼容集合并锁定；不要求使用网页当时显示的最新版。

[R1] Cargo Workspaces — https://doc.rust-lang.org/cargo/reference/workspaces.html
[R2] Cargo metadata — https://doc.rust-lang.org/cargo/commands/cargo-metadata.html
[R3] wgpu API / Web features — https://docs.rs/wgpu/latest/wgpu/
[R4] wasm-bindgen-futures spawn_local — https://docs.rs/wasm-bindgen-futures/latest/wasm_bindgen_futures/fn.spawn_local.html
[R5] OffscreenCanvas — https://developer.mozilla.org/en-US/docs/Web/API/OffscreenCanvas
[R6] Transferable objects — https://developer.mozilla.org/en-US/docs/Web/API/Web_Workers_API/Transferable_objects
[R7] IndexedDB transaction complete — https://developer.mozilla.org/en-US/docs/Web/API/IDBTransaction/complete_event
[R8] Web Audio best practices — https://developer.mozilla.org/en-US/docs/Web/API/Web_Audio_API/Best_practices
[R9] Cargo dependency resolver / feature unification — https://doc.rust-lang.org/cargo/reference/resolver.html
[R10] glyphon — https://docs.rs/glyphon/latest/glyphon/
[R11] wasm-bindgen browser and worker tests — https://wasm-bindgen.github.io/wasm-bindgen/wasm-bindgen-test/browsers.html
[R12] Playwright projects — https://playwright.dev/docs/test-projects
