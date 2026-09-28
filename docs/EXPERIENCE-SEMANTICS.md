# 体验保真能力与语义设计

状态：提案，尚未实现。基线：`8c44ace`。本设计针对导入体验审计后剩余的能力差距，不把已经修复的六项引擎／转换器问题重新列为能力需求。原六份 `docs/specs/` 保持原文；本文是可分阶段落地的补充设计，不能作为当前能力声明。

## 1. 设计结论

增加三类有明确所有者的契约：

1. **作品交互配置**：输入映射、阅读偏好、自动／快进规则。Player 解释操作，Core 只接收经过校验的语义输入。
2. **剧情演出任务**：转场、音频增益与停止包络、对白框显隐动画。Core 持有时间、任务终态和可恢复状态，渲染／音频后端执行投影。
3. **声明式界面**：图片菜单生命周期、有限动画、等待提示、系统页面皮肤与 UI 文案。Player 持有界面实例，Presentation 投影布局和类型化动作。

不引入任意主题脚本、源引擎操作码或第二个剧情 VM。源引擎效果编号、按键码、对象属性和音量单位由转换器规范化；无法确认的映射必须报告，不猜测为完全等价。

当前字体选择、锁定图片、翻页取消语音已经有可用机制，不再为这些问题重复造能力。完整原版体验还取决于字体授权、原引擎默认参数和设备表现；本设计定义可表达性与验证方法，不宣称已完成实机对照。

## 2. Gap 到能力的映射

| Gap | 拟增加或调整的契约 | 主要责任 | 验收重点 |
| --- | --- | --- | --- |
| 舞台空白处不能推进、Space 行为固定 | InputPolicy 与临时隐藏状态 | Player / Engine / Host | 消费一次输入、控件优先、隐藏后首击只恢复 |
| Ctrl 按住快进、文字速度、自动等待不可配置 | ReadingPolicy 与持久化偏好 | Player / Core | Gate 不绕过、失焦释放、语音后等待 |
| wipe 都变成 dissolve | TransitionSpec | Core / Renderer / Compiler | 定义化方向／遮罩、两后端一致、途中恢复 |
| 停止音频无淡出、事件音量烘焙 | Audio.gain、AudioGain、AudioStop | Core / 音频后端 | 连续包络、准确终态、原 PCM 不被事件增益截断 |
| 消息框只能瞬时显隐 | DialogueWindow 任务 | Core / Presentation | 脚本显隐与用户隐藏分离、可等待／恢复 |
| 图片菜单无 BGM、按钮音效、出入动画 | MenuSession 与有限生命周期声明 | Player / Presentation | 资源就绪后进入、退出一次、无跨实例迟到动作 |
| 字体／阴影／等待提示不一致 | FontPlan 配置、TextPaint、Indicator | Compiler / Presentation | 字形和阴影边界、非动画提示兜底 |
| 系统页面布局、选项和 UI 语言不一致 | 封闭 PageSchema 与 UiCatalog | Player / Presentation / Compiler | 保留系统能力和可访问性、UI 与正文语言独立 |

动画图像解码是导入器媒体能力；运行时先接收编译好的帧序列，不要求运行时理解源动画容器。视频、NVL、Ruby、通用滤镜、原存档迁移不由本次 gap 推导为本轮范围。

## 3. 状态、时钟和持久化边界

| 状态 | 所有者 | 时钟 | 持久化／恢复 |
| --- | --- | --- | --- |
| 场景转场、剧情音频包络、对白框透明度 | Core | Story 微秒时钟 | 写快照；恢复当前进度，不重新执行脚本 |
| 当前对白有效揭示间隔、字素进度、Gate | Core | Story | 写快照；当前实例参数冻结 |
| 输入策略、阅读策略、菜单／页面定义 | Program 的已解析配置 | 无 | 进入发行身份；编译和加载均验证 |
| 字速、自动等待、音量、减少动态等偏好 | Player | 无 | 独立偏好存储，不随读档回退 |
| Auto、按住／切换 Skip、临时隐藏、自动等待累计 | Player | Story / 控制事件 | 不写剧情快照；读档／新会话清零 |
| 菜单实例、菜单音轨／动画、焦点／阅读偏移 | Player / Presentation | UI 活动时钟 | 不写剧情快照；设备重建保留当前实例进度 |

UI 活动时钟只在对应界面可见且可交互时推进；后台、设备恢复、该界面必要资源准备期间停止。打开系统菜单暂停 Story，但不能因此冻结菜单自己的进入动画和音频。嵌套页面遮住原页面时暂停被遮页；返回时继续，不能重新播放 enter 音效。

语义先后次序沿用 owner 队列、输入序号和任务 ID。宿主时间先归一化为有界增量，再由所有者推进。音频采用 Story／UI 逻辑时间到设备音频时钟的锚点；恢复后重建锚点，不把 AudioContext 或原生句柄写进快照。确定性指相同输入与完成事件序列得到相同状态，不承诺不同设备的实际输出延迟一致。

## 4. 输入与阅读

### 4.1 InputPolicy

新增作品级输入配置，编译为封闭枚举，不使用浏览器事件字符串作为核心协议：

```text
InputPolicy {
  advance_hit: dialogue | stage_background
  story_bindings: Map<PhysicalInput, ReaderAction>
  hide_restore: next_primary_or_hide
}
PhysicalInput = PrimaryClick | SecondaryClick | Enter | Space | Escape | ControlHold
ReaderAction = Advance | ToggleDialogueHidden | OpenMenu | HoldSkip | None
```

初期只允许表中动作／输入的已验证组合：HoldSkip 仅用于带 down/up 的 ControlHold；指针和按键不接受任意参数。作品默认保持现有 Enter／Space 推进、对白区域点击推进。转换器可选择全舞台推进、Space 隐藏及 Ctrl 按住快进。移动端由可见控件触发同一动作，不模拟 Ctrl。

路由优先级：宿主输入框／模态面板 → 当前可访问控件及焦点激活 → 隐藏恢复 → 当前屏幕绑定 → 舞台背景。PrimaryClick 在控件未命中时才匹配背景；信箱黑边不算舞台。聚焦按钮的 Enter／Space 保持按钮激活，不同时执行阅读动作。按键自动重复不得反复触发切换动作。

输入携带 session、screen_instance、interaction、sequence；一个输入最多产生一个语义动作。页面切换、pointercancel、失焦或进入后台清除按住状态；按下与释放必须对应同一指针／按键实例。菜单与选择界面不能落入 Story 背景推进规则。恢复可见性的一次输入不得穿透为 Advance。

### 4.2 临时隐藏

保留 `Operation::DialogueVisibility` 的脚本含义，新增 `ReaderState.dialogue_hidden_by_user`：

```text
可绘制对白 = 脚本窗口当前透明度 > 0 && !dialogue_hidden_by_user
```

隐藏对白文字、框、名字和等待提示；不隐藏选择、错误或系统逃生入口。用户不能通过“恢复显示”把脚本隐藏的框显示出来。允许隐藏的前提为 Story 中存在对白且无选择／模态层。

临时隐藏时关闭 Auto／Skip，并持有独立的 reader-hidden 暂停 token，暂停 Story 及耦合语音；UI 控制输入仍可用。再次 Space 或主键输入只释放该 token；右键／Escape 打开系统菜单，清除隐藏状态，但菜单的暂停 token 接管，不能出现一帧剧情推进。新会话、读档、返回标题清除隐藏状态。设备重建保留该状态。

这是第一版明确选择的阅读语义。如果某源引擎隐藏期间继续音频／剧情，转换器必须标注差异；不能偷偷改变全局暂停机制。日后有确证需求再增加显式模式。

### 4.3 揭示速度

新增偏好 `reveal_interval_us: Option<Micros>`（None 为作品值，0 为瞬时），作者的 `Dialogue.reveal_us` 仍可表达逐句节奏。新增对白 `reveal_policy: player | fixed`，旧内容默认 fixed 保留行为；导入的普通对白选择 player，特殊演出可 fixed。

Player 在 PendingActivation 中冻结有效间隔：player 使用偏好或作者值，fixed 使用作者值。冻结值进入对白任务／快照；偏好改动对下一实例生效，当前正文、字素位置和 Gate 不重建。即使间隔为 0，也只揭示到第一个尚未释放的 Gate；有界执行预算继续生效。

设置界面的快／慢档位只是 interval 的 UI 投影，不使用依赖帧率的“每帧几个字”。同一字素簇不可拆开；换行不额外产生字素等待。

### 4.4 Auto 与 Skip

```text
ReadingPolicy {
  auto: { mode: parallel | after_voice,
          base_wait_us, per_grapheme_us,
          voice_wait: associated_non_looping | none }
  skip: { allowed: read_only | all, interval_us }
}
Preferences += reveal_interval_us?, auto_wait_us?, skip_unread: bool
Dialogue += completion_voices: [TaskRef]
```

`completion_voices` 在激活提交时解析为真实任务 ID，可引用同 Cue 声音或仍活动的声音。只接受非循环 Voice；不以“总线上任意 Voice 正在响”阻塞当前页。另增 `Operation::DialogueBindVoice { dialogue: TaskRef, voice: TaskRef }`，允许脚本在页内 Gate 事件播放语音后、DialogueContinue 之前追加绑定；按真实任务 ID 去重，关联集合随对白任务保存。操作只允许作用于仍 Running 的对白和已建立的非循环 Voice；后续同名句柄替换不改变旧绑定。转换器必须建立这些关联，未知关联报告降级，不靠总线扫描推测。

令 R 为当前页已全部揭示、Gate 已通过且进入等待推进的时刻；V 为关联语音集合全部终止的时刻（自然结束、显式停止或被替换均解除等待）；无语音时 V=R。D=玩家覆盖或作品 base_wait，加 per_grapheme×当前实例可见文本字素数，不计控制标记，换行不计数。

- parallel：到达 R+D 且语音集合已终止才推进，相当于 max(R+D,V)。
- after_voice：从 max(R,V) 再等待 D，表达“剩余语音结束后再等用户设定时长”。
- 声音失败进入正常错误恢复，不能伪装成完成；looped Voice 被静态拒绝关联，避免无限等待。
- 只累计未暂停的 Story 时间；手动翻阅、隐藏、选择、读档关闭 Auto。切换自动模式和修改自动等待偏好重置本页计时，避免立刻跨页。
- 自动阅读沿用阅读视口跟随已揭示内容；手动长文翻阅仍优先于 VM Advance。

有效 Skip 为 toggle_skip 或 hold_skip；进入任一 Skip 关闭 Auto。read_only 按既有 TextId+MeaningRevision 已读身份判断；all 仍要求玩家显式启用 skip_unread。遇到未授权未读页、选择、暂停、Gate 时不能强制完成任务或伪造 DialogueContinue。Gate 可由正常脚本释放后继续；选择必须由玩家回答。快进每次推进经过同一阅读路由并遵守 interval 和 owner 工作预算；不定义“无条件跳过所有 Await”。

## 5. 剧情演出任务

以下为候选 DTO 形状，不是当前可编译 JSON。Micros 沿用十进制字符串序列化；所有浮点值必须有限，枚举、引用与范围在编译和运行加载时双重验证。

### 5.1 TransitionSpec

```text
StagePresent { scene, duration_us, transition,
               cancel: settle_target | restore_source }
Transition = Cut | Dissolve
           | Wipe { direction: left_to_right | right_to_left | top_to_bottom | bottom_to_top,
                    softness: 0..1 }
           | Mask { asset, softness: 0..1, invert: bool }
```

缺省保持旧 duration=0 的 cut、非零 dissolve。Cut 必须 duration=0。第一阶段仅扩充方向 wipe 与静态灰度 mask；不要把未确认的源 wipe 编号直接映射为同名效果。

场景在激活提交时捕获完整 source／target 和所需遮罩；p=clamp(elapsed/duration,0,1)。方向 wipe 的阈值 q 是舞台归一化坐标：left_to_right 为 x，right_to_left 为 1-x，其余同理。Mask 的 q 来自资源像素的编码灰度 R/255（作为数据读取，不做 sRGB 线性化），invert 时取 1-q；忽略 alpha，灰度一致性由编译器验证。遮罩拉伸覆盖设计舞台，双线性采样，边缘钳制。

p=0 必须全 source，p=1 必须全 target。中间时 softness=0 用 q≤p 的硬阈值；softness=s>0 时，边界 b=p×(1+s)-s/2，权重 w=1-smoothstep(b-s/2,b+s/2,q)。输出为在线性预乘空间中的 mix(source,target,w)。这一定义覆盖方向、软边和端点，测试无需依赖效果名称猜测。

转场独占当前 stage root；并行 StagePresent 或写入被拥有场景的 Clip 拒绝，继续沿用所有权检查。Finished 提交 target；Cancel 按显式策略选择完整 source 或 target，首期不支持保存任意混合结果作为新场景。替换需先终止旧任务，再从确定的已提交场景建立新任务。duration=0 在同次提交中完成，Started/Finished 各锁存一次。

快照保留 source、target、效果参数及 elapsed；资源闭包包含两侧和 mask。尺寸变化、设备重建只重新投影，不重启进度。预算不足走既有准备失败／重试，不自动换成 cut。减少动态仅令视觉投影直接显示 target，逻辑任务仍按原时长结束，避免更改脚本 Await 时序；明确提示该选项不加速剧情。

### 5.2 音频增益与停止

```text
Audio += gain: f32 = 1                 // 首期合法范围 0..4，线性倍率
AudioGain { target: TaskRef, to: 0..4, duration_us,
            easing: linear | smooth,
            replace: bool, cancel: commit_current | restore_base }
AudioStop { target: TaskRef, duration_us, easing: linear | smooth }
```

每任务输出增益 = Audio.gain 经包络调节后的值 × 玩家 bus_volume × master_gain。Mute 的增益为 0，但不结束任务、不释放语音等待。事件增益保留为播放参数，转换器导出未乘事件音量的 PCM；超幅混音可能仍在输出端削波，不承诺增益超过 1 永不失真，也不默认加入改变原动态的 limiter。

AudioGain 是具有自身 TaskId 的包络任务，激活时捕获目标当前增益，对目标 gain 通道独占；replace=true 从当前解析值接续，旧包络 Cancelled，不跳回原始值。Finished 将目标基础增益设为终值；Cancel 根据策略提交当前或恢复捕获前基础值。目标在包络结束前终止时，包络 Cancelled；错误则按既有错误路径传播。

AudioStop 是可 Await 的停止任务；捕获当前增益淡至 0，目标音频在淡出期间仍 Running、仍占资源，直到包络结束才发送物理 stop。停止任务 Finished，目标 Audio 标记 Cancelled，等待 Audio 的旧代码仍走 on_cancelled；需要等“正常完成停止”时 Await AudioStop，而不是 Await Audio.Finished。

AudioStop 取代已有 gain 包络；停止已启动后禁止新的 AudioGain。对同目标重复 AudioStop 拒绝为所有权冲突，针对已终止目标的 Stop 立即 Finished，不复活声音。目标先自然结束时 Stop 立即 Finished。Cancel Stop 提交此刻增益并释放控制，目标继续；Finish Stop 立即执行终点并停止目标。直接 TaskControl Cancel Audio 仍为立即硬停止，同时取消其控制任务。scope 退出／返回标题／会话替换硬停止，不等待淡出阻塞清理；转换器若要淡出，必须在离开 scope 前显式安排并等待 Stop。

同刻处理沿用任务终态规则：先确认目标状态，再结算依赖控制任务，终态只发布一次；迟到的设备 ended 回调不得覆盖 Cancelled 或新任务。停止中的音频、包络和位置进入快照，恢复时从当前 gain／offset 继续剩余时长，不重放整段。

后端按锚点安排 gain ramp，暂停时取消未来自动化并保留解析值，恢复重排剩余段。Web 和 Windows 必须共享包络公式、终态和代次协议。无法启动音频时采用现有手势解锁／错误流程，不虚构播放完成。

### 5.3 对白框显隐任务

新增 `DialogueWindow { visible, duration_us, easing, cancel }`，控制专用 dialogue-window 通道，覆盖文字、框、名字和指示器的整体 alpha。现有即时 DialogueVisibility 相当于取消该通道动画并立即设终点；不改变对白内容、揭示进度或 Gate。

任务从当前 alpha 插值至 visible?1:0；同通道写入必须显式替换。Finished 提交终点；Cancel 支持 commit_current／settle_end／restore_base，含义与 Clip 一致。任务可 Await，非 Await 时允许与揭示并行。快照保存当前基础值、起终值和 elapsed；中间值不强制转回 bool。用户隐藏仅额外盖住投影，不改写该通道。减少动态保持逻辑时长，规则同场景转场。

## 6. 菜单生命周期与 UI 演出

图片菜单升级为 Player 拥有的 MenuSession，不复用 Core 的剧情 frame/session scope，也不调用任意函数实现 hover。已有 Entry 动作仍是通过验证的剧情入口。

```text
ImageMenu += {
  music?: { asset, gain, looped, enter_fade_us, exit_fade_us },
  sounds?: { focus?, activate?, disabled? },
  enter?: [UiTween], exit?: [UiTween]
}
UiTween = { target: WidgetId, property: x | y | opacity | scale,
            from, to, delay_us, duration_us, easing }
```

首期单属性单区间、有限进入／离开轨道；无脚本回调、无变量写入。相同控件属性的重叠轨道编译失败。focus 音效由“焦点目标实际改变”触发，键盘／指针共用，不按每次 mousemove 播放；activate 只在动作被接受后触发，locked 按钮不播放成功音效。

状态机为 Preparing → Entering → Active → Exiting → Closed：

- Preparing 在旧页面保留画面；菜单图片、音频、帧序列全部经原预算和代次校验，成功后一次提交新实例。
- Entering 播放一次进入轨道和音乐；到 Active 才接受作品按钮，期间系统 Escape 可取消进入并退出，不执行故事入口。
- 接受动作后立即锁定实例，准备目标资源；失败保持当前菜单并恢复可重试状态，不先销毁旧界面。准备成功后执行退出轨道／淡出，再提交目标动作一次。
- Exiting 不再接受第二次激活；后台暂停 UI 时钟。系统紧急离开、错误、会话重置可硬取消，不被作品长动画困住。
- 跳入剧情前菜单音频停止并释放租约；默认不把菜单 BGM 偷渡成剧情 BGM。要延续音乐需以后单独设计音轨转移，本期转换器报告这一差异。

MenuSession 音频独立于 Story 暂停，但仍乘同一用户总线音量。Story 打开系统菜单后，原剧情音频保持暂停；关闭后按原位置恢复。设备重建重建可见状态和循环音乐位置，不重放 focus／activate 等一次性声音。迟到回调绑定 menu_instance 与 audio_instance。

资源按当前界面实例、退出中的实例和候选准备持有；禁止把所有菜单媒体常驻剧情。预算失败沿用原子准备与重试，不能因添加菜单音效退回无界缓存。

## 7. 文字、等待提示和系统页面

### 7.1 文字与指示器

字体资源继续用现有 FontPlan；转换器／作者配置目标正文语言、合法字体和 fallback。不得把字体不一致一概解释为缺少新 IR。新增 `TextPaint.shadow { offset_x, offset_y, color }`，首期为单层无模糊阴影；布局尺寸仍按字形度量，裁切／资源成本须包含阴影外扩。描边、多层阴影和滤镜另行设计。源样式编号须有明确映射或标为近似。

新增 `Indicator { anchor, offset, visual: builtin | image | frames }`，frames 为编译好的 PNG 资源序列与逐帧 duration，正时长、有界帧数；运行时不解析外部引擎动画格式。锚点可为框角或当前排版末字素位置，不依赖固定屏幕像素。

仅在当前对白可见、Gate 已释放、文本已揭示且等待用户推进时显示；不把加载／Gate 等待冒充可翻页。静态后备指示器必须对自定义 rect 同样存在，不因作者使用原图消息框而消失。减少动态使用指定静态帧；隐藏时停止 UI 动画更新，无需空转 rAF。恢复不追求装饰动画逐帧存档一致。

### 7.2 系统页面定制

沿用封闭组件契约，新增 title/replay/menu/settings/saves/history 页面槽，允许布局、图片、文字样式和有限 UI 动画；不把任意 UI 树作为可执行脚本。

`PageSchema` 将 WidgetId 绑定到类型化模型／动作，例如 `Setting<RevealInterval>`、`Setting<AutoWait>`、`SaveSlot<SlotId>`、`HistoryList`、`Action<Close>`。玩家值、启用条件、槽修订、选择实例等由 Player 提供，主题不能任意构造或改变授权。列表使用既有测量／裁切／分页能力，不通过图片坐标绕过命中和焦点检查。

每页必须有可访问名称、焦点顺序及返回路径；缺少必要设置／恢复动作时，编译失败或显式使用完整内置页面，不能默默隐藏系统功能。系统错误／存储冲突／发行导航使用保留动作，主题只能改变可允许的外观。任意用户输入和 DOM 面板仍具有最高输入优先级。

UI 文案新增作者 `UiCatalog` 资源，复用现有消息格式和独立 ui_locale 解析；编译检查消息键、参数类型与显式 fallback。正文 ja 支持不等于 UI ja 已实现；增加界面语言必须同时完成目录、FontPlan 覆盖和实际布局验收。语言切换准备新页面文字／字体后原子提交，保持当前正文实例冻结规则。

系统页面原图还原是组件能力和转换映射的共同工作。与 NIR 存储模型不等价的源选项、旧存档格式必须标为 unsupported，不用外观相似宣称兼容。

## 8. 版本、验证和降级

本提案涉及新 Effect、任务快照与运行时字段，不能仅靠 serde default 假装旧运行时可执行。第一批语义扩展采用 source Program v2、indexed runtime v3、Snapshot v2；升级 SDK 同时提供旧源格式的显式归一化入口。旧发行及其存档由原配套运行时继续打开，不跨发行迁移。

复用 Program／RuntimeRoot 已有的 `requires` 和运行时 `CAPABILITIES` 校验，不再新增平行字段。编译器从实际使用的指令和配置求所需能力集，代替目前直接写入全部支持能力的方式；手写输入缺少必要声明也须拒绝。SDK 清单增加可供作者工具检查的对应支持集；编译时校验，加载时再次校验，缺少能力在任何剧情／媒体副作用前报 `E_CAPABILITY`。候选能力 ID 为 `input.policy.v1`、`reading.policy.v1`、`stage.transition.v1`、`audio.envelope.v1`、`text.window.v1`、`ui.menu-lifecycle.v1`、`ui.pages.v1`。具体字段即使未使用，旧严格解析器也可能拒绝，因此不能用能力声明代替版本升级。

偏好独立版本化：旧存储缺少字段时使用新字段默认；未知未来版本保留原记录并回退本次会话默认，不覆盖损坏／未来格式。未知核心字段继续拒绝。

导入策略增加作者显式选择：

- strict：已识别但不能等价表达的可达语义阻止输出完成；未知控制流始终报错。
- approximate：仅允许列入白名单的演出近似，逐项写出源位置、原参数、目标语义和原因。运行时不再二次无声降级。

这些策略是拟新增接口；现有 importer 命令不能宣称已具备。源位置和作品内容只进入作者侧报告，公共测试／设计采用中性样本。

初期限额作为配置验证和预算的双重约束：每菜单最多 256 控件、64 条进入／退出轨道；每帧序列最多 256 帧；UI 同时音频最多 8 路；单次 UI 进入／退出阶段的最长 delay+duration（含音乐淡变）不超过 10 秒。剧情包络和转场最长 60 秒。引用资源仍必须满足原联合账本，数量限额不代表可分配保证。超过限额明确诊断，不截断。限额后续可版本化调整。

## 9. 分阶段交付与验收

| 阶段 | 交付 | 必须通过的验证 |
| --- | --- | --- |
| A 交互阅读 | InputPolicy、隐藏、揭示偏好、Auto／Skip、静态等待提示 | 控件与背景互斥；失焦释放 Ctrl；隐藏恢复不推进；长文／Gate／未读／语音结束的时序表；偏好与读档独立 |
| B 演出 | Audio gain/stop、DialogueWindow、TransitionSpec | 包络采样值；暂停／恢复／替换／目标先结束；转场端点和软边像素；途中读档／回退／设备重建；预算失败保持旧画面 |
| C 菜单 | MenuSession、菜单 BGM／音效、有限动画、帧指示器、阴影 | 连点仅进入一次；候选失败重试；后台暂停；菜单退出无残留音频或租约；键鼠触摸与焦点一致 |
| D 系统页面 | PageSchema、UiCatalog、原图系统页面适配 | 必要操作可达；槽冲突／旧动作拒绝；窄屏／字体放大／双语言；真实导入页面逐项对照 |

每阶段先定义格式、验证器与中性测试，再接 Core／Player，随后同步 Web／Windows 适配和转换器。若某平台尚未实现相应能力，其 SDK 不得声明支持；不能把 Web 通过当作 Windows 通过。

每个新增任务必须覆盖：0 时长、正常结束、取消、Finish、替换、scope 退出、暂停、多次恢复、陈旧回调和预算失败。自动阅读使用受控时钟与合成语音验证精确等待；UI 输入使用实际浏览器事件；音频时序测试与人工可听性分别记录。WebGPU／WebGL2 做同场景像素容差对照，原生音频另测停止与设备时钟。

集成验收继续执行真实浏览器完整主线及全部入口；增加按住快进、Auto、隐藏／菜单反复切换、保存于淡出／wipe 中途等路线。资源峰值仍按账本与物理内存分别记录，不上调预算掩盖生命周期泄漏。原引擎实机对照和物理试听是体验等价的额外证据，不能由静态源脚本或软件渲染回归替代。

## 10. 落地前需补齐的证据

以下不阻止通用契约设计，但阻止对应转换映射被标为 exact：源 wipe 编号的方向／边缘／遮罩公式，隐藏期间的音频与计时行为，源菜单出入动画的参数与打断行为，以及字体／阴影的真实绘制结果。

建议先落实 A，再做 B 的音频与对白框；wipe 映射与 C/D 根据证据逐项接入。第一版不扩展菜单音乐跨域转移、多关键帧通用动画或任意主题逻辑；新增需求应给出独立生命周期与恢复契约。

参考：[当前能力](CAPABILITIES.md)、[架构边界](ARCHITECTURE.md)、[作品主题](PROJECT-THEMES.md)、[阅读视口](AUTHOR-READING.md)、[语言与字体](LOCALE-FONTS.md)、[导入范围](IMPORT.md)。
