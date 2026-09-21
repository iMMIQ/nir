# NIR-0004：编译优化、执行格式与性能验证

状态：设计提案 v0.1。核查日期：2026-09-20。

范围：Web 首版，Rust/WASM 执行与文字/UI，wgpu 渲染；承接 NIR-0001～0003。不涉及旧引擎转换或作者编辑器。本包没有实现 NIR 优化器、Rust 播放器或 GPU 编译器；配置是候选实验配置，不能视为实测最优参数。

## 1. 决策摘要

推荐“专用内容编译器 + 可裁剪的通用 Rust/WASM 运行时 + 离线 shader 检查 + 浏览器端受控预热”。

第一版不把每部作品直接转成 Rust/WASM，不在浏览器引入 LLVM，不把每条剧情变成一个宿主函数。内容和代码分离，优先生成资源、文本、管线和恢复准备数据，然后对 Rust 与 Binaryen 构建配置做受控实验。

三条硬约束：

- 优化不能删除交互、错误、资源屏障、随机数推进、任务生命周期或存档恢复语义。
- 产物更小、开发构建更快、玩家首次进入更快和稳定播放更快，分别计量。
- 编译优化产物必须通过相同内容、相同有序逻辑输入的差分与恢复测试。

## 2. 编译发生在哪里

| 层 | 时间 | 输入与输出 | 主要目标 |
| --- | --- | --- | --- |
| 内容编译 | 发布构建 | NIR → 验证/解析后的模块、依赖和恢复表 | 减少启动解析与播放分析 |
| 原生构建工具 | 开发/CI | Rust 工具 → 构建工具程序 | 缩短内容迭代，不随游戏发行 |
| 引擎代码编译 | 发布构建 | Rust/MIR/LLVM → wasm-ld → WASM | 裁剪、机器无关代码质量 |
| WASM 后处理 | 发布构建 | bindgen 后模块 → 优化模块 | 模块层死代码和尺寸优化 |
| WASM 执行编译 | 玩家浏览器 | WASM → 本机代码 | 浏览器负责，不能由发布机直接完成所有工作 |
| shader 处理 | 构建与浏览器 | WGSL → 验证/变体 → 目标管线 | 避免首用停顿和变体爆炸 |

WebAssembly 字节码不是面向所有设备直接可执行的本机代码。V8 的文档说明了基线编译、热函数重新优化与代码缓存；这是 V8 实现信息，不是所有浏览器都遵守的性能契约。[R7]

WGSL 存在 shader-module creation 与 pipeline creation 等不同阶段，离线语法检查不能替代目标设备上的管线构建。[R10]

首句就绪应测量依赖 DAG 的关键路径，不把重叠进行的下载、编译与资源工作简单相加，也不将平均帧率当作启动指标。

## 3. 内容编译流水线

```text
Canonical NIR + 文本契约 + 资源/组件声明
  -> 受限解析、类型与引用校验
  -> 控制流图、调用图、效果摘要、恢复入口集合
  -> 保守语义优化
  -> 符号解析、局部索引、常量池、执行格式
  -> Cue 依赖配方、文字需求、管线需求
  -> 恢复映射、计费映射、诊断旁表
  -> 独立输出验证
  -> 按模块序列化、hash 与发布
```

保留一个不做优化的参考执行器。参考执行器也必须使用 checked 数值、相同交互模型和任务契约，不应采用另一个简化语义。

模块级优化默认优先于全作品重写。各模块的局部符号表独立，跨模块引用使用稳定身份和版本化接口。不要因为第一章新增一个 TextId，就重排整部游戏的所有整数索引。

第一版可以先对规范 JSON 解析出紧凑内存表示，然后评估离线索引化 JSON 或二进制模块。不需要为采用“二进制”而先建设复杂归档协议。

## 4. 优化正确性契约

### 4.1 可观察行为

比较对象至少包括：

- 控制流选择、剧情变量、Profile 合并和随机数状态。
- 对白身份、冻结插值、选项身份及其提供快照。
- Activate 的提交分组、资源必要性与失败/取消去向。
- 任务、作用域、属性所有权和锁存里程碑。
- StoryTick、Gate、跳过规则、关键演出参数。
- Fault 类型、发生顺序和稳定来源位置。
- 支持保存的切点和其可恢复状态。

不要比较物理 GPU 句柄、内存地址、缓存命中或下载回调排列。运行实例身份若允许内部重编号，测试必须采用保持等价关系的映射；不能因此合并本来不同的实例。最终像素比较单独设置容差与色彩测试，不承诺跨 GPU 位一致。

### 4.2 效果摘要

```text
EffectSummary
  reads / writes: Frame, Session, Draft, Task, Profile, PreferencesSnapshot
  may_fault
  may_suspend
  advances_rng
  creates_identity
  starts_or_cancels_task
  reads_story_or_media_clock
  resource_barrier
  crosses_gate_or_interaction
  observable_at_save_cut
```

没有副作用不等于不会报错。checked 除法、索引、整数溢出即使不修改全局状态，也不能任意删除或提前执行。

NIR 的 checked I32 必须显式实现 checked_add/checked_div 等，不依赖 Cargo release 的 overflow-checks 默认行为；错误走 NIR Fault，不走 Rust panic。[R1][R4]

### 4.3 V1 推荐优化

| Pass | 默认范围 | 必须证明 |
| --- | --- | --- |
| 符号解析 | Stable ID → 局部索引 | 正反向映射完整 |
| 常量折叠 | 类型明确、必成功的纯表达式 | 无溢出、除零或隐式副作用；保留逻辑计费 |
| 常量分支简化 | 条件可证明确定 | 不删受支持恢复入口和其他根 |
| 不可达内容删除 | 完整根集合外可证明不可达 | 不是“测试没走到” |
| 不可变数据池 | 相同参数/只读文本数据等 | 数据共享不合并语义或实例身份 |
| 准备配方编译 | 静态结构与有界依赖 | 运行时仍检查条件、语言、设备和资源屏障 |
| 冷热数据分离 | 诊断与执行分开 | 最小恢复映射留在发布包 |

CSE（公共子表达式消除）、局部临时值删除和块融合作为后续受控 pass。首先要证明该临时值不是规范存档内容、没有隐藏错误、没有跨切点或状态写入、不会改变计费和诊断位置。

### 4.4 不能盲目做的优化

1. 删除结果未使用的 Random：它仍然推进 RNG。
2. 删除结果未使用的 checked_div：它可能报错。
3. 把某条分支的 `1 / 0` 提前变成全游戏加载错误：错误时机和可达性变了。可以保留原执行位置的 Fault，或统一由前置验证政策拒绝；不能只在开启优化后拒绝。
4. 合并相邻 Activate：资源屏障、提交顺序、失败路径与共同起点都可能改变。
5. 删除零时长任务/Await：可能仍分配身份、提交终态或释放所有权。
6. 内联后自动消除调用帧：Frame 任务作用域与返回清理仍然有语义。
7. 删除当前透明/离屏节点：可能仍参与遮罩、布局、命中或将来动画。
8. 删除在默认语言中未使用的 Gate、字体、回退路径。
9. 基于某次试玩的分支频率删除其他路线。
10. 用 approximate math 改写剧情判定、时间、关键 Gate。

### 4.5 保存切点与指令预算

NIR-0001 允许在一致性切点保存，预算切片可能发生在同步运算符之间。因此不能假设“同一个基本块里所有操作不可观察”。

V1 保留规范 Op 边界的逻辑恢复位置和预算累计。即使物理表达式被折叠，也保留原始逻辑计费信息。任务/Session/Draft 写入不得跨可观察切点做死存储消除。

若后续使用 superinstruction，必须保存内部逻辑 sub-PC、可停止位置、原始计费和错误位置；不能从“100 条原指令”变成“只收一次 fuel”。时间切片的物理快慢不是语义，但输入接收协议、故障和暴露给存档的切点必须保持契约。

## 5. 可恢复执行格式

建议执行模块包括：

```text
Header: 语义版本、执行格式版本、模块身份、能力
FunctionTable / BlockTable
TypedSlotLayout
IndexedOps / Terminators
ConstantPool / ReferenceTable
ActivationRecipes
ResumeMap
SemanticCostMap
可选 DebugSourceMap
```

ResumeMap 不是调试符号，可区分：

- 物理执行地址 -> Stable Op/Terminator ID 与恢复阶段。
- 规范局部槽 -> 执行槽或明确的重建规则。
- 调用返回与逻辑帧 -> 执行帧/返回位置。
- PendingActivation / Await -> 当前阶段及冻结参数。

首版保留所有规范可存档局部；只删除编译器内部临时槽。不要先做激进寄存器分配，再假设 Stable ID 自动解决恢复。

存档关联精确内容、状态格式及所需执行产物身份。相同内容在不同优化级别之间恢复，是需要测试的能力，不是自动成立。实现原产物固定或规范化状态映射其中一种，并明确拒绝未支持的组合。

索引化模块仍是外部输入。验证长度、offset、枚举、引用、大小上限和跨模块接口。不得把任意网络字节 transmute 成含 Vec/指针的 Rust 对象；zero-copy 也不能绕过结构验证。

## 6. 最有价值的编译产物：准备配方

### 6.1 Cue 依赖摘要

输出直接依赖、条件依赖、有界候选集合、语言参数和管线种类。必要/可能依赖（must/may）必须分开；候选集不是一次必须全部加载的集合。

遇到动态资源表达式，编译到受限解析配方或有界资源域。无法静态封闭时，不得因为“未发现引用”裁掉资源与解码器。

### 6.2 准备工作何时可以提前

| 事项 | 构建时 | 运行时 |
| --- | --- | --- |
| 图层静态结构、资产身份、样式默认值 | 预解析 | 实例化与动态值 |
| 动画关键帧类型、时长检查 | 预检查 | 激活时捕获初值、按 StoryClock 求值 |
| 文字结构、Gate 契约、字体字符闭包 | 预分析 | 插值、语言/字号/宽度决定的排版 |
| GPU binding/pipeline recipe | 预生成并验证 | 设备、surface 格式、采样数等最终条件 |
| UI 绑定与动作名 | 解析为索引、构建依赖关系 | 状态变化时投影、布局与输入令牌检查 |
| 曲线系数 | 可选确定算法预计算 | 端点和关键时刻精确遵循语义 |

不把动态名字的正文永久塑形成固定字形；不把角色从当前值启动的初值在构建时固定；不烘焙按某台设备尺寸计算的通用 UI。

如果曲线拟合、音频压缩、纹理量化引入误差，归类为有画质/误差预算的资源变体，而非悄悄打开的“严格等价优化”。

## 7. Rust 编译：先削减依赖，再调优化等级

### 7.1 独立构建图

构建工具可以使用解析器、字体/图片处理器、完整诊断和 shader 校验工具。播放器只依赖必要 schema/loader/runtime 与平台实现。

使用：

```sh
cargo tree --target wasm32-unknown-unknown -e features
cargo tree --target wasm32-unknown-unknown --duplicates
cargo tree --target wasm32-unknown-unknown -e features -i wgpu
```

在工作区中应按实际播放器 package 增加 `-p` 选择；不要让构建所有工具的命令代表最终播放器依赖图。[R2][R3]

Cargo features 会合并，顶层 --no-default-features 不会强制关闭其他依赖启用的默认功能。检查真正的目标依赖闭包，而非只数 Cargo.toml 行数。[R2]

### 7.2 专用运行时的边界

可以从整个发行闭包生成 capability report，并选择有限的经过测试的引擎档位。闭包要覆盖所有路线、可读旧存档范围、语言/主题回退和所承诺的后端。

不按玩家这一次进入的场景生成“裁剪证明”；未来内容超过运行时能力时，构建或加载阶段明确拒绝。

第一版不为每部作品生成不同 Rust 源码。先共享同一小型运行时，功能模块确实造成体积压力时才建立少量固定档位，避免指数增长的测试矩阵。

### 7.3 泛型、内联与数据结构

泛型可能在使用位置实例化；单独把依赖的 opt-level 设为 3，不保证相关泛型都在该依赖中得到同样优化。[R1]

建议像素、布局等热点采用具体数据类型；对大型冷配置解析，测量泛型重复实例化成本，再考虑抽出非泛型内部函数。不要全局替换成 dyn，也不要全局强制 inline(always)。inline 是提示，不是保证，过度内联也可能使产物更大。[R5]

频繁场景状态用有界 Vec/arena/整数索引，错误消息冷路径按需格式化；不把每帧的所有状态转成 JSON 穿越 JS 边界。wasm-bindgen 接口尽量粗粒度，同时不人为等待攒批而增加输入延迟。

### 7.4 不能通过 profile 改剧情算术

Rust NIR evaluator 使用 checked 算术和稳定 Fault。关闭 debug_assertions 后依然需要执行不可信模块、存档、资源尺寸和状态边界的真实校验。

wasm32-unknown-unknown 当前默认已采用 panic=abort，显式配置属于固定策略，不能声称相比该默认必然额外减小体积。更极端的 panic 策略、重编标准库和无保护加载不作为首版前提。[R4]

## 8. Cargo 候选构建

配套 Cargo-profiles.example.toml 提供固定 ThinLTO/单 CGU 下的三档：3、s、z。它不是完整工程 manifest，需合入现有工作区根部。

Cargo 文档提醒 s/z 不一定比 3 更小，z 还关闭 loop vectorization；应测试最终产物。只有在第一轮确定大方向后，再测 LTO=off/thin/fat 与 CGU=1/4/16，避免一开始全排列。[R1][R6]

开发和发布分开：日常可用 cargo check、增量编译和较快 profile；正式候选固定依赖与构建环境，禁用增量以形成可比较的最终结果。

不要同时添加 `embed-bitcode=no` 与要求对应 bitcode 的 LTO 配置；直接 LLVM 参数不具备与常规 Cargo 配置相同的稳定契约，缺乏证据时不叠加。[R6]

## 9. wasm-bindgen 与 Binaryen

```text
cargo --locked --profile <候选> --target wasm32-unknown-unknown
 -> 保留原始模块和构建记录
 -> 配套版本 wasm-bindgen --target web
 -> 原样 / wasm-opt -O2 / -Os / -Oz 各独立候选
 -> 最终结构、导入导出和 feature 校验
 -> 验证 glue/Worker/CSP/内存交互
 -> 必要 strip/minify 与再次验证
 -> 最终 hash 与 HTTP 压缩
```

以同一 bindgen 输出分别生成后处理候选，不把每次候选串联建立在上次的输出上。Binaryen 可以做模块层优化，但不理解 NIR 的任务、存档和语义 Gate。[R8][R9]

默认禁止：

```text
--ignore-implicit-traps
--traps-never-happen
--fast-math
--all-features（作为无选择的默认处理配置）
```

前三个会引入额外的 trap/浮点假设；Binaryen 源码明确描述了这些选项改变的假设。不能把“输入通常正确”作为全程序免检查的证明。all-features 也不能证明目标浏览器支持那些特性。[R9]

不要在 wasm-bindgen 处理前盲目删除自定义段。WASM 执行语义可以忽略 custom section，但工具链可能在里面放协议元数据；语言规范可忽略不代表处理工具可忽略。[R15]

最终 strip 后不删除恢复映射、必需导入/导出或模块协议。保留可关联最终 build hash 的调试/符号材料；代码重写后的旧 offset 映射不能冒充最终映射。

## 10. SIMD、线程、PGO 与拆分

### 10.1 SIMD

只在 CPU 像素处理、转码、音频混合、确实足够大的数值批次上评估。VM dispatch、网络和 GPU shader 不会因开启 wasm SIMD 自动整体加速。

WASM 会验证模块内的指令，即使某函数尚未执行。不能把 SIMD 函数和标量函数都放入一个需要在无 SIMD 浏览器运行的模块，再依赖运行时 if 避开。Rust target 文档明确说明这种区别。[R4]

默认一个受控特性基线；收益足够时，使用引导层探测并选择独立增强模块。检测代码本身不能位于只有增强能力才能加载的模块里。

### 10.2 线程

普通 Worker 不等于共享内存 WASM 多线程。共享内存方案需要跨源隔离等部署条件，且会增加同步、工作集和测试成本，第一版保持可选。[R16]

### 10.3 PGO

Rust PGO 的通用流程是插桩、代表性运行、导出/合并 profile、重新编译。[R17]

浏览器目标需要额外验证插桩 runtime、profile 导出方式、工具链与 target 匹配；不能直接照抄原生写文件流程。原生 core benchmark 的 profile 也不自动代表 Web UI/wgpu/宿主路径。

第一版优先内容级 profile：用测试数据调整预取和管线预热顺序，不删除冷分支。输入数据集要覆盖开始、读档、快进、回看、语言切换和大文本；只用静态标题页训练没有意义。

### 10.4 WASM 拆分

最先做的数据拆分是 IR、语言、字体、媒体从引擎 WASM 独立发布。不要 include_bytes 整部游戏。

可选大解码器可以独立 Worker 模块，但需计入重复 runtime、独立内存和传输成本。多实例普通线性内存互不共享，不能把 Rust 指针跨实例使用。

浏览器懒编译可能减少未用函数的首次编译工作，不会把已放在同一下载文件里的字节变成按需网络下载。[R7]

第一版不做通用 wasm-split、动态插件加载或 NIR→WASM 整作 AOT；只有热点证据出现后再分析。

## 11. WGSL 与 wgpu 编译优化

### 11.1 构建时可以做的事情

使用匹配 wgpu 版本的 Naga 解析/校验 WGSL，检查绑定、入口、uniform/storage 布局与支持的语言能力。预生成 CPU 侧 layout 描述并做字段 offset/size 断言，但最终管线仍需由目标设备验证。[R10][R11]

WebGPU 的 module creation 接受 WGSL，不能把发布机生成的 SPIR-V 或 GPU cache 声称为通用浏览器预编译二进制。wgpu 的 ShaderSource 支持多种来源，并不意味着浏览器直接接受每一种来源。[R10][R12]

不把序列化 Naga Module 当成长期内容 ABI；即使采用锁版本内部缓存，也不能绕过目标浏览器的编译和校验。

### 11.2 避免管线组合膨胀

参数分层：

- 语义/算法结构变化：有限 shader 变体。
- 管线创建参数：入口、目标格式、混合、采样、少数 specialization 条件。
- 每帧/每物体参数：位置、alpha、颜色、转场进度，用 buffer/uniform。

不要把每个主题颜色或动画进度作为 override 重建管线。WGSL const、override、运行时值固定于不同阶段，override 改变可能需要不同管线。[R10]

六个独立布尔开关理论上已有 64 种组合；加入格式与采样组合后更加膨胀。构建生成实际受支持的 VariantManifest，并区分首屏、章节首次使用和罕见可选效果。

不建议为每次差异生成全新巨大 shader，也不建议把全部效果塞进一个 mega-shader。以少量稳定基础管线和有界特效模块作为起点。

### 11.3 离线校验不等于首次使用无成本

目标设备上提前创建首屏/近期所需管线。WebGPU 提供异步管线创建接口，但需要核对所用 wgpu Rust 接口的映射，不能直接承诺它暴露完全相同的功能。[R13]

当前 wgpu PipelineCache 文档仅列出 Vulkan 后端，不能作为 Web 持久管线缓存方案。[R14]

WebGL2 回退可能仍需要对应 Naga/GLSL 转换链。不能仅因离线检查了 WGSL，就裁掉回退实际需要的翻译器和验证路径。

shader 测试包含透明边缘、线性合成、遮罩、文本、转场中点、后端切换。改变采样/精度的优化需显式记录画质误差，不能删除 GPU 安全检查来追求速度。

## 12. 编译期资源处理与增量更新

沿用 NIR-0003 的不可变对象与分包：文本/字体/资源不编进引擎常量。编译缓存键包括工具身份、配置、输入与直接依赖摘要，不能只按文件修改时间或源码 hash。

建议：

```text
ArtifactKey = H(tool_identity, pass_config, input_digests,
                dependency_digests, target_profile, data_versions)
```

改变一条英文译文应只影响对应正文及确实变化的字体/索引/包清单，不重新生成其他语言和媒体。

字体子集、颜色/alpha 规范化、mip 与资源变体依赖使用闭包，但保留全部承诺的语言/后端/恢复入口资源。内容合并不应把独立存档身份或配音身份合成同一个身份。

骨架、静态路径几何、样式绑定可提前解析；DPR、动态名字、字体回退、可访问设置和当前演出时刻相关数据仍在运行时处理。

## 13. 工具与报告

| 工具/报告 | 回答的问题 |
| --- | --- |
| cargo tree -e features / --duplicates | 哪些依赖被启用、是否重复 |
| cargo build --timings | 构建关键路径在哪里 |
| wasm-tools validate / print | 最终模块结构与目标 feature 是否允许 |
| Twiggy（兼容时） | 哪些函数或依赖链保留了体积 |
| WASM section 报告 | code/data/custom 等体积分别多少 |
| 最终对象体积报告 | raw、gzip、Brotli 与首屏依赖集合 |
| 浏览器时间线 | 冷启动、首用编译、帧时间、内存 |

wasm-tools 可按 feature 配置验证模块。默认验证通过不等于你的目标浏览器支持，必须显式绑定 feature policy，再跑真实浏览器。[R18]

Twiggy 是体积和可达关系分析器，不是 CPU 热点分析器。与新 WASM 特性的兼容需验证；不能把 native 二进制的体积报告冒充 Web 产物分析。[R19]

开启优化后 strip 冷调试数据，但留 build 对应的私有分析产物。不要把最大函数必然当成 CPU 最慢函数；也不要把 wasm 文件最小当成首屏最快。

## 14. 测试实验设计

### 14.1 分层对比

1. 内容：参考解释器 vs 同一内容的优化执行格式。
2. Rust：固定内容、feature 和 Binaryen 选项，比较 3/s/z。
3. 后处理：固定 Rust 产物，比较无后处理/O2/Os/Oz。
4. 链接：确定大方向后，再改 LTO 和 CGU。
5. 可选增强：独立评估 SIMD、模块拆分和 PGO。

每次只有明确的一组变量改变，不直接全开后宣布哪个选项有效。配套 experiment-matrix.example.json 是分阶段计划，不含实测结果。

### 14.2 语义测试

- 同一输入下终态、关键逻辑 trace 和 Fault 一致。
- 每个可保存切点进行保存、恢复和跨已支持执行档位恢复。
- Random 的结果未使用仍保持随机推进一致。
- 未到达分支内故障不提前触发。
- Activate 准备期间保存后不重复前置状态写入。
- Await 收到取消/失败时结果相同。
- 优化后预算耗尽与切点计费遵循同一政策。
- 动画进度、Gate、语言切换与旧回调失效行为保持。

资源回调顺序可受性能影响，测试通过规范化宿主事件模拟与枚举时序比较；不得拿两个真实网络竞赛的墙上时间差直接认定语义不等价。

### 14.3 性能测试

报告 raw/code/data/custom 字节、首屏压缩依赖字节、获取/编译/初始化分阶段耗时、首帧与首句、文本布局、快进、活跃帧间隔、WASM 内存峰值、资源准备峰值。

同时测冷缓存、HTTP 命中但新进程、正常重复进入与持续阅读。浏览器代码缓存的具体规则不作为引擎正确性契约。V8 的调试与性能采样可能改变编译层级，因此验收应包含不打开 DevTools 的自动化或外部测量，并记录实际浏览器版本。[R7]

内存增长会使普通非共享 WASM 内存的旧 ArrayBuffer 视图失效；手写 JS glue 的视图缓存必须随 memory.buffer 变化更新。allocator 换型也要跑长时间碎片与增长测试，不能仅比较数 KB 代码差异。[R20]

## 15. 开发构建速度

开发用增量/check；CI 可评估 sccache。sccache Rust 文档要求关闭 rustc incremental，且列出了链接型 crate 的缓存限制，不能承诺完整最终链接命中。[R21]

Cargo timings 可以显示单元依赖和构建关键路径。[R22] 建议将重资源 cooking 从每次 Rust build.rs 中移出，建立内容 hash 增量步骤。

正式实验固定 toolchain、Cargo.lock、Binaryen、语言数据、资源参数和环境。记录优化后产物再计算最终 hash；不能在 hash 后继续 wasm-opt 或改写 import。[R23]

## 16. 实施顺序

P0：无优化参考、效果摘要、稳定恢复映射、符号解析与准备配方；真实依赖图；三档 Cargo/后处理实验；WGSL 校验与有限首屏预热；最终产物测试。

P1：局部 CSE/临时槽优化、紧凑二进制模块、容量受限的内容专用档位、增量构建与细粒度资源表。

P2：superinstruction/局部 SSA、真实 WASM PGO、SIMD 增强、按需大模块与更激进特化；每项必须有热点证据和恢复/兼容测试。

不是“默认把所有优化开到最大”，而是“把不变工作前移，把语义边界保留，用最终设备数据决定代码生成”。

## 参考资料

以下为本次核查的主资料；接口与默认值需与项目锁定版本再次对齐。

- [R1] Cargo profiles：`https://doc.rust-lang.org/cargo/reference/profiles.html`
- [R2] Cargo features：`https://doc.rust-lang.org/cargo/reference/features.html`
- [R3] Cargo tree：`https://doc.rust-lang.org/cargo/commands/cargo-tree.html`
- [R4] Rust wasm32-unknown-unknown：`https://doc.rust-lang.org/rustc/platform-support/wasm32-unknown-unknown.html`
- [R5] Rust code generation attributes：`https://doc.rust-lang.org/reference/attributes/codegen.html`
- [R6] Rust codegen options：`https://doc.rust-lang.org/rustc/codegen-options/index.html`
- [R7] V8 compilation pipeline：`https://v8.dev/docs/wasm-compilation-pipeline`
- [R8] wasm-bindgen deployment：`https://wasm-bindgen.github.io/wasm-bindgen/reference/deployment.html`
- [R9] Binaryen optimization options：`https://github.com/WebAssembly/binaryen/blob/main/src/tools/optimization-options.h`
- [R10] WGSL：`https://www.w3.org/TR/WGSL/`
- [R11] Naga：`https://docs.rs/naga/latest/naga/`
- [R12] wgpu ShaderSource：`https://docs.rs/wgpu/latest/wgpu/enum.ShaderSource.html`
- [R13] WebGPU async pipeline：`https://developer.mozilla.org/en-US/docs/Web/API/GPUDevice/createRenderPipelineAsync`
- [R14] wgpu PipelineCache：`https://docs.rs/wgpu/latest/wgpu/struct.PipelineCache.html`
- [R15] WASM binary module sections：`https://webassembly.github.io/spec/core/binary/modules.html`
- [R16] SharedArrayBuffer：`https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/SharedArrayBuffer`
- [R17] Rust PGO：`https://doc.rust-lang.org/rustc/profile-guided-optimization.html`
- [R18] wasm-tools：`https://github.com/bytecodealliance/wasm-tools`
- [R19] Twiggy：`https://github.com/AlexEne/twiggy`
- [R20] Memory.grow：`https://developer.mozilla.org/en-US/docs/WebAssembly/Reference/JavaScript_interface/Memory/grow`
- [R21] sccache Rust：`https://github.com/mozilla/sccache/blob/main/docs/Rust.md`
- [R22] Cargo timings：`https://doc.rust-lang.org/cargo/reference/timings.html`
- [R23] 项目既有设计：NIR-0001、NIR-0002、NIR-0003（本次会话附件）。
