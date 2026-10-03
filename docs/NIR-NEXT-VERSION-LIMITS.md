# NIR-NEXT P0.2 版本与限额矩阵

对应实施计划 §4「P0.2 最小契约记录」与 §12 第 2 条：冻结目标身份、任务结果原因、时间域、源页边界、输入消费、页面动作白名单与 Replay 策略的状态表；冻结数量上限；列出 source/runtime/content/snapshot/preferences/schema/host protocol 的逐项影响。本账本与 [P0 基线账本](NIR-NEXT-P0-BASELINE.md)、[能力发行清单](CAPABILITIES.md)共同构成 P0 完成证据。

每个数值锚定现行代码常量或校验点（括注文件）；文档只记录现状，改动代码须同步改本表。未设上限处如实标注「无独立上限」及其间接约束，不用模糊措辞冒充限额。Windows 原生一律待验证（见基线账本）。

## 1. 版本矩阵

| 层 | 版本/身份 | 锚点 | 说明 |
| --- | --- | --- | --- |
| Source Program 格式 | 1 | `FORMAT_VERSION`（nir-format/src/lib.rs:17） | 源程序 JSON 的格式代次 |
| Runtime root 格式 | 2 | `RUNTIME_FORMAT_VERSION`（nir-format/src/lib.rs:21） | 发行根；拒绝未知能力与未知字段 |
| 内容包 | 2 | `CONTENT_PACKAGE_VERSION`（nir-format/src/lib.rs:22） | 分块内容包 |
| 快照 | 2 | `SNAPSHOT_VERSION`（nir-format/src/lib.rs:23） | v1 快照明确拒绝，不做跨发行存档迁移；v2 新增可选字段（终态原因、带 gain 效果、audio_position_us、audio_device_elapsed_us、窗口揭示、交互游标等）均向后兼容读取 |
| 工程清单 | `project_format = 1` | game.toml（crates/nir-compiler/src/project.rs） | |
| 模块/片段 | `module_format = 1` / `fragment_format = 1` | module.toml、story.nir.json | |
| 场景 | `format = 1` | tests/scenarios/*.toml | |
| 主题 | `format = 1` | theme.toml | |
| 语言配置 | `format = 1` | config/locales.toml | |
| 能力清单 | 47 项（代码 `CAPABILITIES`） | nir-format/src/lib.rs:24 起；scripts/verify_capabilities.py 常设一一对应门禁 | 能力集合只增不改义；新能力必须经源/Runtime 双侧校验与编译器按使用推导 |
| 导入报告 | 格式 2（映射账本 + approximate 计数） | import-report（批次 53） | UI 结构分析报告格式 3（批次 42） |
| 字体工具链 | `nir-font/1;hb-subset/0.3.0;harfbuzz/8.2.2;unicode-normalization/0.1.25` | `FONT_TOOL`（crates/nir-compiler/src/fonts.rs:13） | 工具、数据与实现身份三者进入缓存键 |
| 宿主协议 | `api = "nir-player/0.1"`、`capability_profile = "web-v1"` | crates/nir-compiler/src/project.rs:378-379 | 宿主与运行时必须配套构建；宿主 u32 Tick 分批保留两域余量 |
| 来源二进制 | LSB116 / LPB116 / LPM106 / Gale105-106 | crates/nir-compiler/src/import/lsb.rs、livenovel.rs、media.rs | 其他版本号明确拒绝（E_IMPORT_*），不猜测降级 |
| preferences | 无格式版本号 | crates/nir-player/src/lib.rs:448-453 | 旧记录缺字段自动补默认值并钳制（finite_clamp），新增偏好不升级文件格式 |

升级规则（计划原句的落实）：只有实际变化的格式升级，旧文件缺省行为不改。示例：`dialogue_visibility` 未带 `transition`/`duration_us` 保持立即翻转；`MenuTransition` 未声明 `style` 走既有整层 alpha 渐隐；固定行 HistoryWindow 与连续 HistoryFlow 并存。运行时对未知能力拒绝而非静默忽略；schema（schemars）随格式同步再生成，SDK/CLI 配套重建。

## 2. 语义状态表（冻结项）

### 2.1 任务终态原因（N01）

TaskState 保持 Running/Finished/Cancelled/Failed 四态；终态携带原因：正常完成、媒体自然结束、显式完成（Finish 政策提交终值）、显式取消（Cancel 政策，停止任务提交当前值/设备检查点值）、替换（同句柄新实例取代旧实例）、scope 退出（interaction/scene/session 级联）、失败。迟到事件不改写已有终态；Await 分支优先级失败 > 取消 > 完成（组合归并同序）。契约见 [AUDIO-SEMANTICS.md](AUDIO-SEMANTICS.md)。

### 2.2 目标身份

- 播放实例：效果句柄在提交时解析到具体实例，回调携带（域, 会话, 任务）与代次；同名句柄复用不转向旧回调（音频域自批次 5、前台域自批次 13 同构）。
- 补间目标四域（批次 4/56/58）：SceneNode（x/y/scale/opacity）、DialogueRoot（整体/背景/文字 opacity）、ViewElement（opacity/scale/offset_x/offset_y，进入边界动画）、AudioInstance（gain 0–1 包络乘子，叠加在事件 gain × 总线音量之上；与 audio_stop 包络所有权互斥，只允许 linear）。

### 2.3 时间域

Story 与 Foreground UI 两个逻辑时钟；前台/Story/后台/设备丢失/队列溢出的暂停令牌相互独立，交错解除不互相释放；前台 UI 时钟经 `ForegroundClockToken` 租约驱动（上限 MAX_TASKS，完成/取消释放；令牌不可得时立即呈现/立即提交，不产生隐形死页）。页面效果状态、元素动画轨道属前台域瞬态，不入故事快照。契约见 [TIME-DOMAINS.md](TIME-DOMAINS.md)。

### 2.4 源页边界（N04）

对白分页三分：源交互页（Interact 终结）、源页（作者 Cue，页内 Gate 后经 `dialogue_continue` 继续同一对白）、布局视口页（长文滚动分页，翻页不当作源页完成）。契约见 [READING-SEMANTICS.md](READING-SEMANTICS.md)。

### 2.5 输入消费

动作事件携带 action/interaction/sequence/session 四元身份；被接受的动作推进 `last_input`。菜单控件凭据为 (instance, revision, control)：每次接受的提交使 revision +1，陈旧凭据一律拒绝——不存在可重放的控件输入。`SelectChoice` 是对挂起交互的观察（无输入身份、不推进序列）。恢复的交互/对白重铸交互身份，陈旧拒绝由会话纪元承担。

### 2.6 页面动作白名单

`ImageMenuAction`（nir-format/src/lib.rs:1398）恰为以下变体，宿主不得绕过：PushMenu、Back、Reading、HistoryPage、SaveSlot、LoadSlot、Close、AdjustPreference、ToggleReducedMotion、SetLocal、NewGame、Saves、Settings、Title、Menu、Entry、Replay、ExitReplay。无直接 ui_action 的七个（PushMenu/Back/SetLocal/SaveSlot/LoadSlot/HistoryPage/Reading）经 MenuControl 在 Player 侧解析；服务可用性（Back 需深度>0、PushMenu 需深度<上限、SaveSlot 需 can_save 且非回想非忙碌、LoadSlot 需槽存在等）在投影与提交双重复查。

### 2.7 Replay 策略状态表（N09）

| 相 | 进入条件 | 期间不变量 | 离开 |
| --- | --- | --- | --- |
| entering | Replay 动作通过凭据/守卫校验；冻结原会话（快照/检查点/屏幕/菜单面），候选 Core 以 ReplayEntry 屏障独立推进 | 单一活动 Core 不变；冻结页保持屏幕 | 媒体就绪→active；资源失败→保留冻结页可 Retry；准入失败→整事务作废；标题/NewGame→显式放弃 |
| active | 候选媒体准备完成，会话自增、音频重置 | `profile_merge` 不落玩家 Profile；Save/Export 拒绝；Load/Import 拒绝；嵌套 replay/存储控件双重复查即死亡 | 函数 outcome 或 exit_replay→returning |
| returning | 冻结会话作为 Restore 候选重新验证/准备 | 候选准备期间原状态保持 | 提交后新会话+新菜单实例/版本（冻结前输入全部过期）；Rollback 仅无事务或 active 后允许 |

`exit_replay` 非活动相幂等。契约见 [REPLAY-SEMANTICS.md](REPLAY-SEMANTICS.md)。

### 2.8 存档事务（N08）

槽位 0–2（共 3 个）；保存携带 expected_revision 做 CAS，空槽以 revision 0 直接提交；已占用槽先进入一次性确认令牌（绑定 session、页面 instance、发起控件、槽位版本，任一变化即失效）；槽位列表刷新进入菜单 revision。读档 job 绑定 slot/session/页面 instance，迟到回执不改写当前会话。契约见 [STORAGE-SERVICE-SEMANTICS.md](STORAGE-SERVICE-SEMANTICS.md)。

## 3. 数量上限矩阵

### 3.1 任务、场景与输入

| 项 | 上限 | 锚点 |
| --- | --- | --- |
| 并发任务 | 256（MAX_TASKS） | nir-format/src/lib.rs:92 |
| 快照任务 | ≤ 512（MAX_TASKS×2，含终态保留） | nir-core/src/restore.rs:66 |
| 帧缓冲 | 64（MAX_FRAMES） | nir-format/src/lib.rs:93；restore.rs:65 |
| 场景节点 | 1024（MAX_NODES，scene/draft 各自） | nir-format/src/lib.rs:94；restore.rs:67-68 |
| 输入字节 | 16 MiB（MAX_INPUT_BYTES，源 JSON 与资源单元各自） | nir-format/src/lib.rs:91；restore.rs:172/187 |
| 运行内存账本 | 256 MiB（MEMORY_LEDGER_LIMIT） | nir-player/src/lib.rs:29（批次 54 提额并记录峰值依据） |
| 组合嵌套深度 | ≤ 8；单 Cue 叶子 ≤ 256（MAX_TASKS） | 批次 50 校验 |
| 零时长连锁 | 每次派生消耗预算单位；不收敛即 E_LIMIT | 批次 50 |

### 3.2 菜单与页面（N08）

| 项 | 上限 | 锚点 |
| --- | --- | --- |
| 元素总数 | buttons + elements + 滚动条×3 ≤ 256 | nir-format/src/menu.rs:254-262 |
| 文字承载预算 | 64（Text/TextButton/Toggle/Range 各 1；历史窗口计 min(limit,65)） | menu.rs:240-253 |
| 元素层级深度 | 父链 ≤ 8 | menu.rs:466-472 |
| 导航父页 | 每上下文 ≤ 8（MAX_MENU_PARENTS）；导航目标名 ≤ 128 B | menu.rs:4；批次 46 |
| 局部值 / 故事导出 | 各 ≤ 32（键 ≤ 128 B） | menu.rs:878-886 |
| 枚举值 | ≤ 32 个，每值 1–256 B 且互异 | menu.rs:892-899 |
| 显隐/启用条件 | 每列表 ≤ 16 | menu.rs:917 |
| 几何 | rect 分量有限且 \|v\| ≤ 8192、scale 0.01–8、opacity 0–1 | menu.rs:285-297 |
| Stack 容器 | gap 0–1024；可见行累计 ≤ 8192；行 rect\[1\]=0 且高 >0 | menu.rs:326-345 |
| 文本 | 菜单 Text ≤ 4096 B；控件标签 1–1024 B；元素/资产/变量 id ≤ 128 B | menu.rs:289/445-457/590 |
| Range 控件 | min<max、0<step≤max−min、(max−min)/step ≤ 1000、thumb ≥1 且 <宽度 | menu.rs:352-374 |
| 历史窗口（固定行） | 行 1–16、行高 16–1024、字号 8–128、偏移局部 Int 0–999、翻页 \|delta\| 1–16 | menu.rs:425-443、951-959 |
| 历史窗口（连续） | 可见 3–64 行、行高 ≤512、间距 0–1024、滚/翻步长 1–8192；每页一窗口一滚动条 | menu.rs:403-424、266-283 |
| 历史记录 | 会话内 ≤ 1000 条（超限逐出最旧；恢复拒绝 >1000） | nir-core/src/vm.rs:1794/2654 |
| 存档槽 | 0–2 共 3 个（CAS 版本号） | nir-player/src/lib.rs:3133/3182 |

### 3.3 页面效果与音频（N07/N09）

| 项 | 上限 | 锚点 |
| --- | --- | --- |
| 页面转场渐隐 | 0 < fade_us ≤ 2 s（带样式时必须 >0） | menu.rs:591-598 |
| 页面音乐/音效资产 id | ≤ 128 B；音乐 gain 有限 0–4 | menu.rs:590-609 |
| 元素进入动画轨道 | ≤ 128 条；时长 (0, 2 s]、延迟 ≤ 2 s；同元素同属性单轨；from 界限 opacity 0–1 / scale 0–8 / 偏移 \|v\| ≤ 4096 | menu.rs:611-637 |
| 事件 gain / 停止/补间/窗口揭示时长 | gain 有限 0–4（页面音乐同）；补间与停止时长 ≤ 60 s（零时长合法，保留精确端点语义）；窗口揭示 0 < 时长 ≤ 60 s | nir-core/src/validate.rs:2718 等；2693/2983/3354/4085/4244 |
| 实例增益补间 | to 0–1、仅 linear；与 audio_stop 包络互斥 | 批次 58 |
| 前台时钟租约 | ForegroundClockToken ≤ MAX_TASKS（256），完成/取消释放 | 批次 48 |

### 3.4 表达式、导入与字体

| 项 | 上限 | 锚点 |
| --- | --- | --- |
| 导入表达式归一化 | 指令 ≤ 64、展开节点 ≤ 256、文本 ≤ 16 KiB | crates/nir-compiler/src/import/ui_expr.rs:8-9、144-148 |
| UI 结构分析预算 | 对象/属性 + 数据流共 4096 条 | 批次 30-31 |
| 转换媒体总量 | ≤ 1 GiB（E_IMPORT_LIMIT） | livenovel.rs:1873 |
| 来源字速 | StatusTextSpeed 0..=640 ms/字（×1000 换算 µs；0 为瞬时） | 批次 60 |
| 生成字体子集 | ≤ 64 MiB（E_LIMIT）；缓存条目带 64 B 摘要信封 | fonts.rs:285/316 |
| 偏好 | text_speed 0.25–4（默认 1）、auto_wait_scale 0.25–4（默认 1）、font_scale 0.8–1.5（默认 1）、三音量 0–1（默认 0.3/0.8/0.5）、reduced_motion 布尔 | nir-player/src/lib.rs:448-453（越界/非有限值钳回并补默认） |

### 3.5 未设独立上限的项（如实记录）

- 函数/块/Cue/选项个数：无独立计数上限，受 16 MiB 源输入与运行任务预算间接约束。
- 菜单页数与 push 链长度：单菜单受元素预算约束；跨菜单父页链有 ≤8 上限（见 3.2）。
- 对白正文长度：无每页字符上限，受模块 JSON ≤16 MiB 与文本批次预算（菜单侧 64，见 3.2）约束；呈现层可见 run 预算 64（历史流）。
- 快照总字节数：无显式字节上限，受任务/帧/节点/资源单元各项上限约束。

## 4. 逐项影响清单（P0.2 第三组）

| 层 | P0 期间实际变化 | 处置 |
| --- | --- | --- |
| source | 新增能力字段的操作（tween 目标、Sequence/ParallelAll、窗口揭示样式、类型化 result/on_cancel、E_INFINITE_WAIT 诊断） | 均为可选字段 + 能力门控；旧源文件语义不变 |
| runtime | 能力集合扩至 47；快照 v2 可选观测字段 | 未知能力拒绝；未知字段 deny_unknown_fields |
| content | 内容包版本保持 2 | 无破坏 |
| snapshot | 保持 2（v2 内新增可选字段向后兼容） | v1 依旧明确拒绝 |
| preferences | 新增 text_speed/auto_wait_scale/reduced_motion 等 | 无格式版本；旧记录补默认值（finite_clamp） |
| schema | 随格式同步再生成（批次 4 起补齐 module-code 等历史缺口） | verify_capabilities.py 常设门禁 |
| host protocol | 分域 Tick（TickDomains）、设备位置观测、AudioEnvelope 检查点、Save/Load job 回执 | 宿主与运行时配套重建；旧单域 Tick 保留兼容包装 |

完成证据：本文 + docs/NIR-NEXT-P0-BASELINE.md（fixture 清单与基线测试记录）+ docs/CAPABILITIES.md（能力执行/恢复/后端证据）。数值改动必须同批更新本表与对应契约文档。
