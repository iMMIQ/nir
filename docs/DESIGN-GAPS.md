# 六份设计文档与 v0.1.0 实现差距

核对日期：2026-09-21。代码基线：已发布的 `v0.1.0`，提交 `4213d8324f717aaf5cf7d8cb77fa6cdf8f238fac`。`docs/specs/` 中六份文件与用户提供的原文件逐字节一致。

本报告依据设计正文、实际类型/执行路径、构建脚本和现有测试证据。没有把设计附件中的示例或“建议”当成新增执行指令，也没有为本次对照重新运行测试。最近一次发行验证为 45 项原生测试、10 项 Chromium 浏览器测试通过。

“部分”表示有工作实现但未覆盖设计的完整契约；“未实现”表示当前没有对应能力；“未验证”不等于没有实现。文档明示后置或条件评估的能力单独说明，不将它们统称为 V1 必须完成项。不提供没有统一验收权重的完成百分比。

本文保留 v0.1.0 基线结论；后续修复与验证见 [引擎稳定性进展](ENGINE-STABILITY.md)、[请求生命周期](REQUEST-LIFECYCLE.md)、[结构化诊断及测量](DIAGNOSTICS.md)、[作品配置和主题契约](PROJECT-THEMES.md)、[作者阅读与预览](AUTHOR-READING.md)及[独立语言上下文与字体计划](LOCALE-FONTS.md)。下表不能直接作为当前分支的未完成清单；NIR-0003 中的基础 UI/正文独立选择与简中/英文 FontPlan 已在后续阶段实现，语音/格式区域、多文字系统，以及 UI／配音／区域数据的独立分包仍未完成。

当前多模块工作已推进到 M2.2：发布运行时 v2 的模块静态定义、代码、逐语言正文和资源目录可独立驻留、驱逐及重新加载；活动执行与候选恢复通过租约保护，存档原始定义按单元校验，默认开启有界内容预取。接口与验收见 [模块工作流](MODULE-WORKFLOW.md) 和 [分块内容与驻留契约](CONTENT-RESIDENCY.md)。以下仍为原始 v0.1.0 对照，不改写历史结论。

M2.3 增加规模测量和确定性门禁：正式编译的 3／32 章作品、预取开关配对、固定网络条件，以及超过 16 MiB 的独立合成容量场景；覆盖重入、回退、存读档和清理。CI 软件渲染与本机真实 GPU 分开记录，耗时暂不设未经基线支持的通过阈值。测量方法见 [诊断说明](DIAGNOSTICS.md#m23-多模块基准)，实测证据见 [M2.3 验证](validation/m23/README.md)。这一步没有新增媒体 lookahead、通用准备调度或物理内存测量。

启动与跨章优化在 M2.3 基础上并行下载已验证发行中的启动对象，为 WASM／JS／JSON 提供可复现的 gzip 传输旁文件，并将章节批次下载接入全局四槽并发池。完整性检查、原子交付、预取提升及原预算保持不变；取消后的最后一个内容消费者等待底层验证结束后才释放暂存；返回标题时保留旧画面并等待标题媒体就绪再绘制，同时保留历史与对白 Gate 的布局更新。回归中还修复了暂停音频准备等待 resume 而停滞的路径。关闭 Playwright trace 的前后对照与剩余问题见 [启动与跨章优化验证](validation/startup-content/README.md)。

## 当前推进顺序（2026-09-24）

1. **维护规模基线并据此选择优化。** M2.3 已提供启动、章节等待、回退／恢复和驻留趋势测量，以及预算、清理、剧情等价和重载门禁。启动并行、压缩传输与章节并发已完成；下一步依据关闭 trace 的真实 GPU 对照继续检查主线程／渲染与剩余长尾。持续保留相同设备和工作负载的样本，耗时硬阈值另行建立。固定次数的章节路线不等于数小时长稳验收。
2. **按测量推进准备调度。** 当前预取只沿有限的确定控制流寻找一个后续模块，限于静态定义、代码和当前语言正文；通用资源 DAG、媒体 lookahead、解码／上传按帧预算，以及成本和期限优先级仍需独立实现与验收。
3. **继续保留独立验收项。** 物理内存、Worker／WebGL2、优化执行格式、资源 cooking 与增量构建不由 M2.2 的完成推导为已实现。

内容预算统计唯一对象的编码字节，默认 16 MiB；单对象／单批次最多 16 MiB、单批次最多 128 个对象。准入可原子驱逐无租约对象，仍无法容纳时保持旧视图并失败。恢复另用最多 16 MiB 的独立临时视图逐单元校验原始定义，累计校验闭包可超过缓存预算；最终活动状态仍须满足正常预算。预取仅占空闲缓存，最多 2 MiB，不驱逐已有内容。以上均不是进程物理内存上限。发行运行时仍为 v2，源格式及存档版本仍为 v1；存档仍要求精确发行匹配。

## 总体判断

当前是一个能创建作品、编译、发布和实际游玩的单模块实现。确定性核心、Rust/wgpu 正常界面、基本演出、存读档/回退、双语示例、SDK/CLI、内容寻址发行和工程依赖边界已有实现。

距离六份设计的完整目标，主要还缺：可扩展的作品/主题契约、多模块与语言分包、完整语言上下文、通用资源调度、Worker/兼容后端、编译优化及性能验证、自动质量门禁。另有几项基础工程契约也只做了简化实现，不能都归因于“高级功能留到以后”。

## NIR-0001：核心语义、演出与 UI 契约

已有：Bool/I32/String、checked 表达式、函数/返回、确定赋值、固定随机流、主要 Op/Terminator、PendingActivation、Await/All、Gate、稳定选项身份、可恢复基本任务和检查点。

| 设计范围 | 当前差距 | 状态 |
|---|---|---|
| §2 模块与身份 | 只允许一个模块；没有跨模块导出链接、模块加载边界与模块级恢复 | 未实现 |
| §3 类型系统的建议扩展 | 只有 Bool/I32/String；没有 Enum、受限集合及作为值使用的类型化资源/任务引用 | 未实现；属于基础类型之外的扩展 |
| §8 场景节点与变换 | Group/Sprite 采用同一个 Node 描述；没有 TextSurface、Mask、Camera、rotation/pivot、独立 x/y 缩放 | 部分 |
| §8 组与转场 | Group 透明度沿子树相乘，没有隔离组整体合成；转场主要是整场景 cut/dissolve，没有通用 root_scope、遮罩与转场完成/取消政策 | 部分 |
| §9 Clip | 当前是一条属性从捕获初值到终值的插值，支持 x/y/scale/opacity 与两种 easing；没有通用多关键帧、向量轨、循环、add 通道及内容声明的 skip 政策 | 部分 |
| §10 文本与对白 | 有 Text、Break、Param、Gate 和简单 emphasis；没有 Ruby、独立非阻塞 Marker、通用 Style、追加/NVL 页面、完整 reveal/read policy | 部分 |
| §11 UI slot 与主题 | 正常 UI 已由 Rust 绘制；没有剧本可选择的 UI slot、受验证 Props/action 组件契约和可替换页面 | 部分 |
| §5/7/13 音频任务 | 有 BGM/Voice/Sfx、完整缓冲播放、暂停和位置恢复；没有音轨 Marker、独立 VoiceKey/take、可配置循环区间/包络和正式时钟锚点/延迟观测 | 部分 |
| §15 诊断 | 有稳定错误码和逻辑位置；没有完整 source_ref、文件行列、任务/资源上下文及多个同刻失败的诊断集合 | 部分 |

依据：`crates/nir-format/src/lib.rs` 的 ValueType、Effect、Node、Span、Theme；`crates/nir-core/src/vm.rs` 的 Task、Dialogue、Snapshot；`crates/nir-core/src/validate.rs`；`crates/nir-platform-web/host.js` 的音频适配。

## NIR-0002：加载与渲染

已有：真实 WebGPU、准备失败保留旧画面、联合预算准入、不可复制 ReadyLease、请求去重、基础缓存释放、全句排版缓存、线性预乘合成、静态画面停止 GPU 提交及设备重建。

| 设计范围 | 当前差距 | 状态 |
|---|---|---|
| §2 后端与线程目标 | 无 WebGL2 兼容路径、Runtime Worker、Asset Worker、OffscreenCanvas 探测与主线程同协议回退 | 未实现；本轮计划明确排除 |
| §3 首屏路径 | WASM 全部读取并校验后才实例化；整个 Executable 和两种正文一起加载。没有流式编译、按章节/选定语言启动和恢复入口的最小下载闭包 | 部分 |
| §4/5 需求 DAG | 目前为当前资源集合和固定准备阶段，宿主最多四个并发资源循环；没有 Required/Near/Speculative/Background 调度、lookahead、期限/成本优先级及通用依赖 DAG | 部分 |
| §4/7 缓存与准入 | 有去重、pin、预算和按当前引用释放；没有完整多层淘汰策略、预测取消、资源质量变体重选及物理分配成本回报 | 部分 |
| §6 资源路径 | PNG/PCM WAV/字体；没有 WebP 等可选编码、浏览器 ImageBitmap 快速路径、mip/图集 cooking、多分辨率变体；KTX2/Basis 属于设计中的条件优化 | 部分 |
| §8 上传调度 | 图片在资源回调中同步解码并整张上传；没有按帧字节/时间预算、分块上传，以及 UploadEnqueued/OrderedUseReady/WarmupCompleted 分层观测 | 未实现完整调度 |
| §9 热路径与绘制 | 有持久顶点缓冲和基本管线，但仍逐 quad 绘制；没有相邻兼容实例合批与完整脏 revision 系统。转场复用离屏纹理，却每个实际转场帧重画两侧，没有“一次冻结绘制、随后只混合”缓存 | 部分 |
| §11/12 调度与清晰度 | 有最终尺度 UI、DPR 上限、静态休眠；没有完整动态分辨率/质量滞回策略。打字机仍以 rAF 检查时间，没有按下一字素期限单独唤醒 | 部分 |
| §12 音频 | 无长 BGM 流式/分块方案；恢复主要依赖逻辑 elapsed 和 AudioContext 暂停，不是完整的 Story↔Audio 时钟映射与误差测量 | 部分 |
| §15 观测 | 没有完整请求—下载—解码—上传—租约—提交时间线、GPU 时间、长稳帧分布和指定网络/设备下的 P95 门槛验证 | 未完成验证体系 |

全文件哈希校验是当前启动缓冲的实际原因；不能在保留同样校验保证时直接宣称已实现零等待流式启动。KTX2、复杂缓存和更激进优化需有测量依据，不应仅为补齐名词而实现。

依据：`crates/nir-assets/src/lib.rs`；`crates/nir-platform-web/host.js` 的 prepare；`apps/player-web/host/bootstrap.js`；`apps/player-web/src/lib.rs` 的 resource/draw；`crates/nir-render-wgpu/src/lib.rs` 的 upload_rgba/render。

## NIR-0003：国际化、分包与发行

已有：Fluent 简中/英文 UI、正文契约与 Gate 顺序验证、当前对白/选项/历史内容冻结、下一对白应用语言偏好、精确发行身份、SHA-256 对象、存档导入导出、子路径启动。

| 设计范围 | 当前差距 | 状态 |
|---|---|---|
| §2 四种语言设置 | Preferences 只有一个 locale，同时驱动 UI 和后续正文；没有独立 UI/text/voice/formatting 设置及对应 requested/resolved 上下文 | 未实现完整模型 |
| §2 语言解析与回退 | 仅支持 zh-Hans/en，浏览器语言是硬编码匹配；不是完整 BCP 47 校验、保留 script 的显式回退图和按偏好顺序协商 | 部分 |
| §3 文本修订 | TextContract/TextDoc 共用一个 revision；没有分开的 MeaningRevision、source_revision、contract_revision、contract_digest 和翻译复核流程 | 部分 |
| §3 UI 消息与配音 | Fluent 已使用，但尚无通用消息参数类型契约；配音直接引用固定音频资产，没有按语言的 VoiceKey/take/替代目录 | 部分 |
| §4 字体与排版 | 固定示例字体与基本覆盖检查；没有按 locale/script 的 FontPlan、章节增量/动态名字 fallback、区域格式化数据和日文/繁中正式支持 | 部分 |
| §5 切换事务 | 两种正文已预装，保留当前实例的边界规则有效；没有远程语言包 Resolve/Reserve/Prepare/PendingReady/Commit 完整状态机。无效语言拒绝测试不能代替下载失败、连续切换竞态测试 | 部分 |
| §6 独立分包 | 正文等仍嵌在同一个 Program 对象，FTL 内嵌 SDK；没有独立 UI/正文/配音/本地化图片/字体计划/locale-data 包及按需差量获取 | 未实现 |
| §8/9 发布产物和托管 | 有静态目录、MIME、缓存策略和本地 CSP；没有 HTTP Brotli/gzip 发布协商、明确最终 WASM feature policy、多环境独立 SDK 重建证据及 iframe 支持验证；本地服务器目前明确禁止被 iframe 嵌入 | 部分 |
| §10 上线与回滚 | 本地 stable 指针最后原子替换已实现；没有 staging 公网检查/晋级、并发发布保护、客户端新版本提示和旧发行保留管理工具 | 部分 |
| §11 离线/PWA | 无章节/语言/配音离线选择、闭包完整性索引、CacheStorage staging 和 Service Worker 更新协议 | 未实现；文档列为 P1 |

存档目前只接受精确相同发行，这是首版允许的明确兼容政策；“没有跨版本迁移”不能单独算作违反 V1。若要支持版本升级/回滚后继续读旧存档，仍需新增迁移/旧版本获取与完整兼容测试。

日期标签目前由宿主直接格式化，尚没有玩家独立的 formatting locale。RTL、泰文、复杂 Indic、竖排均未正式支持或验收；设计本身也要求分别验收，不能当成已经承诺所有语种。

依据：`crates/nir-format/src/lib.rs` 的 Preferences/TextContract/TextDoc/ReleaseManifest；`crates/nir-player/src/lib.rs` 的 Locale action；`crates/nir-presentation/src/lib.rs` 的 Messages/TextEngine；`crates/nir-platform-web/host.js`；`crates/nir-compiler/src/release.rs`。

## NIR-0004：编译与优化

已有：内容验证、运行时连续指令表、地址索引、ResumeMap、SemanticCostMap、基础资源配方、固定 Rust/依赖/bindgen、播放器与编译器依赖隔离。

| 设计范围 | 当前差距 | 状态 |
|---|---|---|
| §3/5 执行格式 | 发布的 Executable 仍带完整 Program 及附表；运行时重新验证/展开。变量、函数等仍大量以字符串/BTreeMap 引用；没有完整 TypedSlotLayout、常量池、引用池、紧凑 IndexedOps 和冷热分离 | 部分 |
| §4 效果摘要与优化 | 无 EffectSummary、常量折叠、常量分支简化、不可达内容删除、不可变数据池；也没有独立优化执行器与参考执行器差分链路 | 未实现 |
| §5 恢复/计费映射 | 已有并校验简单稳定地址映射，所有语义成本为 1；没有经过重写/槽优化后的恢复、内部 sub-PC 和多执行档位兼容测试 | 部分；后半依赖优化功能 |
| §6 准备配方 | 当前是 Cue→资源集合，没有 must/may、条件依赖、有界候选、语言参数、布局与管线准备的完整配方 | 部分 |
| §8/9 编译实验 | 固定 opt-level=s、ThinLTO、单 CGU；没有 3/s/z、Binaryen O2/Os/Oz 对照实验和性能/体积选择证据 | 未实现 |
| §9/11 产物校验 | 无独立 wasm-tools feature-policy 检查、构建期 Naga/WGSL 校验与 VariantManifest；实际浏览器运行和 wgpu 管线创建校验已有，不能混为同一件事 | 部分 |
| §12 增量资源处理 | 没有 ArtifactKey 构建图、通用内容增量缓存及翻译/字体/媒体细粒度失效。现有相同对象复用，不等于增量编译 | 未实现 |
| §13/14 报告与基准 | 没有 WASM section/保留链体积分析、raw/gzip/br 首屏闭包报告、冷/暖缓存和长期性能实验集 | 未完成 |

SIMD、共享内存线程、WASM PGO、superinstruction、局部 SSA、通用 wasm-split 和整作 NIR→WASM AOT 也未实现，但文档将其列为条件增强、P2 或明确不做的首版方向，不应作为当前基础缺陷。

依据：`crates/nir-compiler/src/project.rs:389`、`crates/nir-format/src/lib.rs:566`、`crates/nir-core/src/validate.rs:16`、`apps/player-web/src/lib.rs:63`、根 `Cargo.toml` 和 `xtask/src/main.rs`。

## NIR-0005：工程架构与运行协议

已有九库三入口、真实 Cargo 依赖检查、同步纯核心、播放器协调、ReadyLease、按原因暂停、候选读档、独立存档回执和设备恢复。这份文档的模块骨架完成度较高，但以下协议不能标记为完整落实。

| 设计范围 | 当前差距 | 状态 |
|---|---|---|
| §6 回调只入队 | 普通资源/音频/存储回调经 host.deliver 直接调用 Engine.resource/audio_ended/host_event，再同步调用 Player.pump。只有设备重建期间临时缓存回调；没有统一有界 inbox | 部分；需要补齐基础契约 |
| §6/8 一轮工作预算 | Player.pump 把同一 budget 分别交给每个事件及后续推进，没有一轮共享剩余预算；解码、上传也没有统一工作预算。Core 时间推进还使用内部固定 run 预算 | 部分；不能把 API 参数视为完整预算落实 |
| §8 有界账本 | 缺少关键事件终态槽预留、过载政策、输入/遥测分类合并；没有完整跨消费者取消与外部 AbortController 生命周期 | 部分 |
| §10 暂停所有权 | 已实现不同原因互不解除，但存储是 BTreeSet<String>；同一个原因的多个持有人不能分别取得/释放独立 PauseToken | 部分 |
| §7 宿主协议 | 无 Worker Hello/Ready、协议版本/能力握手及可转移对象协议 | 未实现；关联后续 Worker 目标 |
| §12 错误与追踪 | Diagnostic 主要是 code/location/message，其他层多为字符串；没有统一分层错误、完整关联字段、恢复选项与端到端阶段追踪 | 部分 |
| §13/14 自动门禁 | 本地检查与测试脚本已有；仓库没有 CI 工作流、定期性能趋势、跨浏览器/后端矩阵、系统性长稳和随机时序故障测试 | 未实现完整门禁 |

这并不证明当前所有普通回调都会产生错误，但它确实不满足设计的“完成回调只写 inbox、由 owner 在有界轮次统一处理”。现有文档中“事件队列”“暂停所有权”等概括需要按上述实际范围理解。

依据：`crates/nir-platform-web/host.js:21`、`apps/player-web/src/lib.rs:89`、`apps/player-web/src/lib.rs:294`、`crates/nir-player/src/lib.rs:136`、`crates/nir-player/src/lib.rs:447`、`crates/nir-core/src/vm.rs` 的 advance_time、`crates/nir-format/src/lib.rs:47`。当前没有 `.github/workflows/`。

## NIR-0006：作品工程与作者流程

已有独立作品、实际 SDK 身份锁、init/resolve/doctor/check/dev/build/test、严格路径与重复 ID 检查、生成 Schema、素材来源、双语示例及两条逻辑用例。

| 设计范围 | 当前差距 | 状态 |
|---|---|---|
| §3 配置责任 | 根清单只支持简化输入；没有 shared、独立 player/locales/web 配置、runtime_preset、完整舞台语义字段及最终配置来源报告 | 部分 |
| §4 构建与锁 | 实际 SDK/CLI 身份已锁；没有更细的资源处理器/语言数据配置身份、增量工作图及 SDK 更新兼容性差异报告 | 部分 |
| §6 模块契约 | 仅单模块，缺共享声明/多模块链接、跨模块导出验证和按需加载 | 未实现 |
| §7 资源处理 | 目录和实际媒体检查已实现；没有 role/pipeline、源 color_space/alpha 声明和通用 resize/mip/编码/字体处理流水线。示例生成脚本不是用户作品的通用资源编译器 | 部分 |
| §8/9 配音与主题 | 无 voice catalog、Speaker 声明体系、主题继承/slot/component 契约。Theme 当前只有五种颜色 token，不能替换对白/选择组件或页面 | 部分 |
| §10 Edition/Profile/Deployment | 仅 edition=full；无试玩入口/终点与依赖泄漏检查。profile=dev/release 虽被接受，但调用同一个 build，尚无不同政策；没有独立部署配置模型 | 部分；参数存在不代表功能完成 |
| §11 dev 体验 | 一次构建后启动服务器；没有文件监听、自动重载、安全热更新、编译错误保留有效预览并标记过期。dev --scenario 只验证已登记用例，并未驱动浏览器进入该用例状态 | 部分 |
| §12 测试隔离 | 有逻辑 ID 用例和 dev 存储后缀；没有同一作品的开发实例/用例独立命名空间和完整主题/语言/质量测试矩阵 | 部分 |
| §13/14 诊断报告 | 有 Schema 和错误码；没有完整源文件行列/引用链/修复建议、资源依赖链与兼容性变化报告 | 部分 |

CG 鉴赏/隔离场景回放未实现，文档明确允许第一版不做。资源注册/改名助手、导航/更丰富编辑诊断属于 P1；高层语法、可视化编辑器、在线协作、公共内容市场属于 P2，不能与基础 CLI 可用性混为一谈。

依据：`crates/nir-compiler/src/project.rs:14`、`crates/nir-compiler/src/release.rs`、`tools/novelc/src/main.rs:142`、`tools/novelc/src/main.rs:158`、`crates/nir-format/src/lib.rs:545`、`crates/nir-platform-web/host.js:15`。

## 有实现但尚未充分验收的范围

- 物理 GPU、移动真机、Firefox/Safari、其他操作系统 CLI、读屏器实机：未验证。现有浏览器证据来自 Chromium + SwiftShader 软件 WebGPU；触摸为浏览器仿真。
- 半透明边缘、线性转场中点、隔离合成与字体的定量黄金图像测试：未形成完整体系。当前截图像素检查不能替代这些色彩/合成验收。
- 配额耗尽、多标签页同时保存完整压力测试、数小时阅读/回退/设备恢复后的内存与碎片趋势：未执行。
- 公网托管压缩/CORS/CSP/iframe、A 会话运行中上线 B、回滚、新旧存档和语言包单独更新：缺完整端到端矩阵。
- 首句 P95≤4 秒、已准备输入到提交 P95≤50ms、活跃帧超时比例：没有在固定网络/设备、多次采样下验收。现有报告为单次实测，且启动计时未覆盖全部导航/WASM 下载。
- 参考/优化器差分、不同优化档保存切点、完整声明能力组合：尚无对应验证；不能用现有 55 项测试代替六份规范的一致性认证。

## 补齐顺序建议

1. 先补基础契约：统一有界 inbox、整轮工作预算、多 owner 暂停 token、错误与阶段追踪；把这些加入 CI 回归。
2. 补作品规模化：配置/主题组件契约、完整修订模型与独立语言上下文、多模块和语言/配音分包、Edition 与真实 Profile。
3. 补准备与平台：通用资源 DAG、预取/分层缓存/上传预算、Worker 和 WebGL2，并用硬件/移动目标验收。
4. 用基准推进编译优化、资源 cooking/增量构建、压缩与部署晋级；PWA、KTX2、SIMD/PGO 按真实需求和测量分别立项。

这与此前收窄后的“可玩端到端首版”范围不同：Worker/WebGL2/Ruby/NVL/多模块/PWA 等当时已明确排除；但队列、总预算等基础契约的简化仍应补齐，不能仅用范围排除解释。
