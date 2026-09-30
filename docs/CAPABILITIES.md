# 首版能力表

此表说明当前实现，附件中其他条款不会被默认为支持。未知指令、未知核心字段及非 `web-v1` 配置均拒绝加载。多模块契约与验证范围见 [模块工作流](MODULE-WORKFLOW.md)。逐能力的执行/恢复/真实后端验收证据见文末 [能力发行清单](#能力发行清单)，由 `scripts/verify_capabilities.py` 随 `cargo xtask test` 核对。

| 类别 | 当前支持 | 边界 |
|---|---|---|
| 逻辑 | Bool/I32/String、局部槽、纯表达式、checked 算术、函数/返回、Branch/Switch/Goto | 无浮点剧情变量、脚本扩展或任意 JS |
| 操作 | Assign、Random、DraftPatch、TaskControl、DialogueContinue、DialogueVisibility、ProfileMerge | Profile 为布尔事实的单调集合 |
| 终结 | Call/Return、Activate/Await/Interact、End/Fault | 单剧情流；跨模块通过具名导出调用 |
| 任务 | frame/session/scene/interaction scope、锁存 Started/Marker/Finished、失败优先于取消的 All、Sequence/ParallelAll 有限组合 | 无 Runtime Worker |
| 模块 | 多模块命名空间、导出链接、共享变量、函数体/正文哈希分包、执行与恢复前准备 | 静态目录与媒体按需准备、有界预取、租约保护及驱逐；无 Edition 或跨发行存档转换 |
| 图像 | Group/Sprite、层次顺序、裁切、cut/dissolve、方向擦除/Alpha 阈值遮罩、x/y/scale/opacity 动画 | 来源 PNG，打包默认转有损 WebP（质量 92，alpha 通道无损保留）；无旋转、滤镜、视频和独立 Group 混合模式 |
| 正文 | 注册字体、样式强调、参数隔离、换行、字素簇揭示、span Marker/Gate、已揭示长文翻阅 | 无 Ruby、NVL、富网页标记 |
| 翻译维护 | 源/契约/语义修订、契约摘要、已读语义身份、逐文本状态、显式复核、旧源迁移 | 简中/英文、逐模块修订；不迁移跨发行存档，详见 [文本修订](TEXT-REVISIONS.md) |
| 字体编译 | 静态 OTF/TTF/TTC face、subset/full、UI/正文逐语言有序 FontPlan、覆盖检查、共享字体字集去重、塑形闭包、内容缓存、许可打包 | 无可变/彩色字体、运行时补字；详见 [字体说明](AUTHOR-FONTS.md) 与 [语言/字体计划](LOCALE-FONTS.md) |
| 选项 | 稳定 OptionId、可见/可用表达式、默认超时、交互实例校验 | 按实际文字高度排版和裁切滚动；旧实例和重复输入丢弃 |
| 界面 | 标题、对白、选项、菜单、设置、回看分页与单条长记录翻阅、存读档、自动、已读快进 | Fluent 界面内嵌在 SDK |
| 作品配置/主题 | `web-standard`、player 默认设置、字段来源报告、dialogue.main/choice.main 内置组件替换 | 原图按钮菜单、有序图文/分组裁切/透明命中区、解锁入口和消息框图片／舞台坐标；无任意组件或可执行主题脚本 |
| 语言 | zh-Hans/en 界面，zh-Hans/en/ja 正文，独立偏好与字体计划；候选准备后原子切换；正文下一实例生效，已存在的对白/选项/历史冻结身份 | 正文按模块/语言获取并校验；不提供日文界面、繁简自动回退或多文字系统认证；详见 [语言/字体计划](LOCALE-FONTS.md) |
| 音频 | PCM16 WAV、循环 BGM、短音效、合成测试语音、总线偏好与每事件 gain（0–4）相乘、手势解锁 | 来源 WAV，打包默认非循环资产转 MP3 CBR；无流式播放、真人配音，可听性需要人工设备检查 |
| 存储 | 按游戏/profile/发行隔离的三槽 IndexedDB、事务确认、修订冲突、导入导出、历史发行入口、独立偏好/Profile | 快照要求相同发行身份；无云同步 |
| 恢复 | 候选先验证/准备、暂停提交、检查点回退、设备重建 | 无安全热更新 |
| 发行 | 实际 SDK/CLI 身份锁、固定发行启动入口、stage/verify/promote/rollback、本地与 URL 校验、来源/体积报告、打包媒体优化 | 无 PWA、签名/CDN 调度 |
| 工具 | minimal/web-basic 模板、init/resolve/config/doctor/check/dev/build/test、text status/update/review/migrate/recover、Schema、架构检查 | dev 监听、候选构建与完整重载；CLI 本次产物为 Linux x86_64 |
| 外部引擎导入 | 同一 novelc 二进制内的 LSB 116 检查、基础控制流／对白；LiveNovel 配置支持事件、原图菜单、回想、GAL 与 WAV/Ogg 转换 | 实验性、配置有范围限制；不支持归档解包、任意动态表达式／自定义事件、动画／视频与旧存档迁移，详见 [导入说明](IMPORT.md) |
| 平台 | WebGPU/WebGL2 自动选择、响应式、键盘/指针/触摸语义 | 桌面 Chromium 双后端、Firefox WebGL2 验收入口；Windows/Linux 原生构建与 CI 验收；Android 原生为实验性（交叉编译与 APK 结构/签名验证，无真机验收）；iOS/Safari 后续安排 |

资源账本、准备配方和缓存提供首版所需的分层准备与有界准入；已加入函数体与正文的跨模块按需获取；没有实现附件中完整的通用 DAG 调度、任意资源类型与高级缓存策略。静态声明目录已分包按需加载，字体仍为逐语言计划。v0.1.0 的实际测试列在 TEST-REPORT.md，后续有界事件队列、共享预算、独立暂停令牌、取消和分块上传的验证见 [引擎稳定性进展](ENGINE-STABILITY.md)；逐请求终态预留、迟到存读档回执及交错压力测试见 [请求生命周期进展](REQUEST-LIFECYCLE.md)。

结构化错误、作者来源定位、显式启用的脱敏阶段追踪与重复测量见 [诊断说明](DIAGNOSTICS.md)。部分旧工具错误仍没有精确来源；GPU 时间、物理内存和真实硬件性能尚未验收。

作品默认设置和主题契约见 [作品配置说明](PROJECT-THEMES.md)。

长对白、大量选项和开发预览的操作与边界见 [作者阅读与预览](AUTHOR-READING.md)。

M4 的发行操作、存档隔离、后端选择和验收入口见 [发行与桌面渲染](M4-RELEASE.md)。

体验保真相关的输入／阅读、演出任务和界面扩展见 [能力与语义设计提案](EXPERIENCE-SEMANTICS.md)。该提案不能视为全部已实现，逐批交付状态见 [实施进度](NIR-NEXT-PROGRESS.md)。

下一阶段的依赖顺序、代码差距与验收门禁见 [NIR-NEXT 实施计划](NIR-NEXT-IMPLEMENTATION-PLAN.md)。该计划基于新的迁移语义提案；已交付与待实施项见实施进展，不能将整个计划视为当前能力。

NIR-NEXT 首批实现状态与存档版本变更见 [实施进展](NIR-NEXT-PROGRESS.md)；其余计划项尚未完成。

音频事件增益、有限淡出停止、终态与恢复规则见 [音频语义](AUDIO-SEMANTICS.md)。淡入、作者总线动画和声明式菜单媒体仍属后续计划。

类型化场景/消息框属性动画、与旧 Clip 的兼容及绘制边界见 [属性动画语义](TWEEN-SEMANTICS.md)。UI 页面目标尚未交付。Story/Foreground UI 的时钟、暂停令牌和音频路由基础见 [时间域](TIME-DOMAINS.md)，页面效果 owner 仍待后续挂接。

对白与具体语音实例关联、Gate 后绑定和 Auto 等待策略见 [阅读语义](READING-SEMANTICS.md)。已提供玩家字速/等待偏好、Ctrl 按住快进，以及共享的背景点击和主键推进判定；已提供独立临时隐藏、显式 Story 暂停政策及自定义对白框静态提示；已接入共享键盘焦点导航，已提供滚动边界跟随与窄屏设置页验收；长选项专项、原生实机与来源隐藏政策认证仍待实施。

非默认玩家隐藏政策使用 `player.hide-policy.v1`，只支持显式暂停整个 Story；默认仅隐藏呈现并关闭自动/快进，不停止音乐。源与 runtime root 均验证声明，详见 [阅读语义](READING-SEMANTICS.md)。

对白正文可选 `text.shadow.v1` 单层有限偏移阴影，跟随正文揭示、裁切、透明度和玩家隐藏；详见 [阴影语义](TEXT-SHADOW-SEMANTICS.md)。不含模糊、多层或来源样式自动推断。

`stage.wipe.v1` 提供四方向、有限软边的场景擦除，保留旧 StagePresent 生命周期与恢复，见 [场景转场语义](STAGE-TRANSITION-SEMANTICS.md)。纹理阈值遮罩见下一项；消息/UI 根仍待实现。

`stage.mask.v1` 提供 Alpha 纹理阈值遮罩、反向极性及有限软边，使用最近邻/clamp 数据采样并进入准备、预算与恢复闭包。RGB 灰度来源须转换为 Alpha 数据，目前没有自动来源映射；消息/UI 根与 live 输入仍待实现。

`ui.menu-elements.v1` 增量支持有序图片/文字、分组变换与裁切、图片按钮和透明命中区，沿用旧菜单动作与 Profile guard，见 [有限菜单元素语义](MENU-ELEMENTS-SEMANTICS.md)。局部状态、值控件及服务绑定见下列能力；通用集合仍待实施。

`ui.menu-state.v1` 提供有界 Bool/I32/枚举局部状态、有限条件显隐/启用、整段文字绑定和单字段赋值。引擎校验页面实例/版本/控件并保持稳定焦点，见 [菜单局部状态语义](MENU-STATE-SEMANTICS.md)。尚不含 params、通用服务模型或故事内 View 存档。

`ui.menu-services.v1` 可用原图菜单接管剧情 Menu 页面，按当前页准备/释放媒体；提供只读数值偏好文本、有限增减设置与减少动态效果切换，复用既有偏好服务和持久化。见 [自定义剧情菜单与偏好服务](MENU-SERVICES-SEMANTICS.md)。值控件、存读档/历史模型见下列独立能力；通用集合与隔离 Replay 仍未完成。

### ui.menu-storage.v1

自定义菜单可用有界固定/局部槽选择器绑定槽位标签，并以 `save_slot` / `load_slot` 请求既有存档服务。覆盖确认绑定页面、控件、槽位版本和一次性令牌；槽位模型变化刷新菜单 revision。依赖既有菜单服务语义；实际使用时编译器同时保留 `ui.menu-services.v1`。详见 [存档服务语义](STORAGE-SERVICE-SEMANTICS.md)。不包含截图预览、扩大槽数量或通用集合模板。

### ui.menu-history.v1

自定义只读历史窗口与实例化翻页服务。每窗口最多 16 行、每页文本总数最多 64；局部偏移有界，保留历史冻结语言及页面作用域内稳定行 key。复用历史快照，不触发旧章节正文加载。详见 [历史窗口语义](MENU-HISTORY-SEMANTICS.md)。不包含语音重播或通用可交互集合模板。

### ui.menu-values.v1

有界 Range/Toggle、类型化局部/偏好绑定、原图变体及共享输入校验。详见 [值控件语义](MENU-VALUES-SEMANTICS.md)。来源系统菜单的自动映射尚未完成，不能据此声明保真迁移。

### player.auto-delay-policy.v1 / text.voice-timer.v1

前者允许固定 Auto 等待（含零等待），保留旧作品按文本长度计算的默认策略。后者扩展现有 DialogueVoice 为 sampled_remaining：一次采样语音余量与当时 Voice 音量，冻结计时截止点，依赖已知音频时长。两者独立声明，编译器按实际使用保留；见 [阅读语义](READING-SEMANTICS.md)。来源 LiveNovel 转换器自动发出配置/绑定，系统设置页的绝对时间滑条映射仍待完成。

`ui.menu-reading.v1`：菜单中的 Auto、已读跳过和临时查看剧情画面。前两者关闭菜单后启用阅读模式；后者保留菜单状态和暂停，恢复时回原菜单。需同时使用 `ui.menu-services.v1`，由编译器按实际动作保留；见 [菜单服务语义](MENU-SERVICES-SEMANTICS.md)。

`ui.menu-stack.v1` 提供有限声明子项的纵向排列，隐藏项不占位、禁用项占位，绘制与命中共享坐标；见 [菜单纵向排列](MENU-STACK-SEMANTICS.md)。`ui.menu-reading.v1` 的可用性可作为显隐/启用条件，沿用版本校验；不等同于自动绑定原引擎状态。

`ui.menu-text-button.v1` 提供与菜单请求身份绑定的文字按钮，支持显式普通/悬停/禁用颜色及作者字体覆盖检查，见 [菜单文字按钮](MENU-TEXT-BUTTON-SEMANTICS.md)。不提供运行时系统字体查找或来源菜单自动适配承诺。

`ui.menu-story.v1`：菜单显式导出已声明 bool/i32 剧情变量，用于只读显隐／启用条件；最多 32 项，提交前复查和 revision 失效，不提供 UI 写剧情通道。见 [菜单故事只读条件](MENU-STORY-SEMANTICS.md)。

连续历史窗口使用独立能力 `ui.menu-history-flow.v1`：真实字体测量、页面内冻结历史、连续裁切和带页面／布局版本的滚动请求；旧固定行窗口保持原能力与语义。预算、输入、宿主支持及来源迁移限制见 [连续历史窗口](MENU-HISTORY-FLOW-SEMANTICS.md)。

`ui.menu-history-scrollbar.v1` 提供绑定连续历史窗口的作者图片轨道、滑块和箭头，显式普通／悬停／按下／禁用图片、持续拖动及垂直键盘语义。复用窗口位置和页面／布局版本，见 [图片历史滚动条](MENU-HISTORY-SCROLLBAR-SEMANTICS.md)。LiveNovel 草稿在校验后自动拆分来源状态条并生成原图历史页，完整格式器和分页语义仍待迁移。

`ui.menu-history-availability.v1` 提供只读 `history_available` 条件，查询当前剧情历史是否非空；不冻结正文，不要求历史窗口。共享显隐、布局、命中和提交校验，事实改变使旧 revision 失效，见 [连续历史窗口](MENU-HISTORY-FLOW-SEMANTICS.md)。

`ui.menu-effects.v1` 提供页面边界的声明式呈现效果：进入/关闭的一次性音效与有限渐隐（≤2 秒，ForegroundClockToken 驱动）、接受提交的点击音效和前台域循环页面音乐；效果音频随页面图片进入准备与留存，Preparing 空窗从不发声，关闭把退出变成锁输入的有限事务。状态瞬态、不入故事快照，会话重置随宿主域重置终止。见 [菜单页面效果](MENU-EFFECTS-SEMANTICS.md)。不含逐元素动画、效果等待或来源系统菜单的自动效果映射。

`ui.replay.v1` 提供显式声明的回想事务：`replay` 控件动作冻结原会话（快照、检查点、菜单页与局部值、auto/skip），候选 Core 在屏障外独自准备后切换为唯一活动会话，结束（outcome 或手动 `exit_replay`）时冻结会话作为恢复候选重新验证并原样接回。活动期间 Profile 写入、保存/导出、读取/导入隔离，嵌套入口与存储动作在派发点复查即拒绝；入口媒体与冻结会话联合准入，重叠资产不重复计费；准入失败整事务作废且不提供 Retry。旧 `entry` 动作行为不变。见 [Replay 事务](REPLAY-SEMANTICS.md)。不含共享变量写回、sleep/awake 语义或来源系统的自动回想映射。

`ui.menu-chrome.v1` 允许页面关闭自动添加的导航按钮，保留作者控件、Escape／右键和失败出口。默认开启，旧页面行为不变；转换器可在原页面只使用键盘返回时自动生成该声明，见 [菜单服务](MENU-SERVICES-SEMANTICS.md)。

`ui.menu-navigation.v1` 提供最多八层父页的 push_menu／back，保存有界局部值、逐层 Close 和返回后的新输入实例；旧 menu 替换行为保持兼容。资源、服务与布局状态的边界见 [菜单子页导航](MENU-NAVIGATION-SEMANTICS.md)。

### task.compose.v1

Cue 内可声明 `sequence` / `parallel_all` 有限组合：组合是一个自主任务，主 VM 停驻于等待、选项或内容屏障时链仍自行前进；子项经与顶层效果完全相同的派生路径获得作用域继承、所有权检查与启动时刻属性捕获。结果归并失败 > 取消 > 完成，已完成副作用不回滚，零时长链逐派生计入执行预算且无限循环不可表达。子项不得为 StagePresent/Dialogue，不得被故事按名等待或控制（Await/TaskControl 只认顶层效果 ID），嵌套深度 ≤ 8、单 Cue 叶子数 ≤ 256；能力裁剪、媒体根与激活配方遍历全树。链中途存读档/回退只续播不重播。见 [有限任务组合](COMPOSE-SEMANTICS.md)。不含剧情副作用、条件子项或来源动画序列自动映射。

### story.typed-result.v1

Interact 可声明 `result` 目标变量与 `on_cancel` 取消路径，选项以 `value` 携带常量值：VM 校验后由唯一 VM 写入声明值再走分支，宿主只报告选项 id；超时按显式选择 default 提交其声明值；取消跳转 `on_cancel` 且不写任何值，未声明取消路径的交互是模态的。类型化交互携带语义选择游标（default 行，缺省首个启用行）进入快照，悬停与键盘焦点保持呈现瞬态；源与 Runtime 校验目标变量、逐选项值类型与取消目标块，恢复对 values/selected/result 声明一致性做权威校验；效果树任何位置的循环音频使自然 Finished 不可达，一律 E_INFINITE_WAIT。见 [类型化交互结果](TYPED-RESULT-SEMANTICS.md)。不含故事内文本输入、IME 或来源交互的自动映射。

### media.webp.v1 / media.mp3.v1

打包媒体优化按实际发出的对象容器裁剪 `requires`：发行中存在 WebP 对象才声明 `media.webp.v1`，存在 MP3 对象才声明 `media.mp3.v1`；`--no-optimize` 或逐资产例外导致发行不含这两类对象时不声明，旧运行时仍可加载，播放器据此拒绝不支持的组合。默认构建把图像对象转有损 WebP（质量 92，alpha 通道在 ALPH 块中无损保留）、非循环音频转 MP3 CBR；尺寸、`duration_us` 和 `decoded_bytes` 描述符保持源资产值。MP3 对象带 LAME gapless 标签，原生加载器与浏览器 `decodeAudioData` 都按标签裁剪编码器延迟/填充，解码样本数与源 WAV 一致（原生加载器经测试逐样本对齐）。循环播放的音频保持 WAV（样本精确循环），MP3 不能表示的采样率、转换后不缩小的对象和字体不受影响；逐资产例外用 catalog `optimize` 字段，CLI 覆盖与转换缓存见 [编写与维护作品](AUTHORING.md)。

## 能力发行清单

每个能力只有在执行、恢复与真实后端验收都有证据时才保持发行（P6.3）。`scripts/verify_capabilities.py` 随 `cargo xtask test` 运行，逐项核对：代码 `CAPABILITIES`（crates/nir-format/src/lib.rs）与本表必须一一对应，表中引用的测试/规格文件必须存在，Windows 原生在真机验收通过前一律记为「待验证」，不得据此声明实机通过。执行/恢复证据为 Rust 测试；Web 后端为浏览器 E2E 规格的实际执行（WebGPU/WebGL2 自动选择）；个别能力的浏览器覆盖为间接（经 fixture 实际运行而非直接断言），在备注中如实标注，不当作直接断言。

| 能力 | 执行证据 | 恢复证据 | Web 后端 | Windows 原生 | 备注 |
|---|---|---|---|---|---|
| module.lazy.v1 | crates/nir-player/tests/runtime_content.rs（百模块增量加载） | 同文件（冷恢复、逐出重载）；crates/nir-core/tests/streaming.rs（分阶段恢复等价） | tests/browser/modules.spec.js（按需取模块/预取/跨章存档） | 待验证 | v0.1.0 基线；nir-next fixture 均为单模块 |
| control.v1 | crates/nir-core/tests/compose_contract.rs（TaskControl 整链终态）；crates/nir-core/tests/audio_contract.rs（Cancel/Finish 终态互异） | crates/nir-core/tests/audio_contract.rs（拒绝伪造终态/旧快照版本） | tests/browser/player.spec.js（跳转/分支/赋值）；tests/browser/modules.spec.js（Call/Return 跨章） | 待验证 | v0.1.0 基线；所有规格隐式经基础控制流 |
| stage.sprite.v1 | crates/nir-presentation/src/lib.rs（scene_tests 图层合成/嵌套裁切） | crates/nir-core/tests/semantics.rs（快照含场景节点值，37% 处恢复） | tests/browser/player.spec.js、tests/browser/backends.spec.js（精灵绘制截图） | 待验证 | v0.1.0 基线；无专属精灵图恢复测试 |
| stage.dissolve.v1 | crates/nir-format/src/transition.rs（端点/覆盖数学）；无专属 Core 执行测试 | crates/nir-core/tests/tween_contract.rs（快照伪造转场互换被拒） | tests/browser/player.spec.js（转场中途掉线恢复）；tests/browser/backends.spec.js | 待验证 | v0.1.0 基线；默认无参转场，证据为机制级 |
| clip.scalar.v1 | crates/nir-core/tests/semantics.rs（类型化轨道保留旧 Clip 路径）；crates/nir-core/tests/tween_contract.rs（Clip/Tween 所有权互斥） | crates/nir-core/tests/semantics.rs（动画中途恢复） | tests/browser/player.spec.js（stay 路线精灵 y 动画） | 待验证 | v0.1.0 基线 |
| tween.target.v1 | crates/nir-core/tests/tween_contract.rs（对白通道独立/能力门） | 同文件（动画中途存读档恢复、拒绝非有限/过期属性态） | tests/nir-next/tween.spec.js（像素+暂停冻结） | 待验证 | UI 页面目标尚未交付 |
| text.structured.v1 | crates/nir-player/tests/coordination.rs（混行尾样式保持揭示偏移、长文已揭示翻阅） | crates/nir-core/tests/semantics.rs（时钟切片保留揭示快照） | tests/browser/authoring.spec.js（换行/翻页/滚轮/触滑/回流/恢复） | 待验证 | v0.1.0 基线 |
| text.revisions.v1 | crates/nir-compiler/tests/texts.rs（源编辑→显式复核→旧源迁移） | 无运行态（仅校验）：同文件（运行时重查摘要）；crates/nir-core/tests/semantics.rs（恢复校验已读身份） | tests/browser/texts.spec.js（修订全链） | 待验证 | v0.1.0 基线 |
| text.gate.v1 | crates/nir-core/tests/reading_contract.rs（Gate 上绑语音不消费） | 同文件（恢复钉住 Gate 绑定实例） | tests/browser/authoring.spec.js（冻结于作者 Gate） | 待验证 | v0.1.0 基线 |
| choice.v1 | crates/nir-core/tests/semantics.rs（过期/重复选择拒绝）；crates/nir-player/tests/coordination.rs（有界命中区） | crates/nir-core/tests/semantics.rs（损坏选择续块被拒） | tests/nir-next/typed-result.spec.js；tests/browser/player.spec.js（双结局+过期拒绝） | 待验证 | v0.1.0 基线 |
| audio.buffer.v1 | crates/nir-core/tests/audio_contract.rs（自然结束不被迟到事件改写） | 同文件（偏移时钟保活恢复）；crates/nir-player/tests/coordination.rs（设备包络按会话恢复） | tests/nir-next/audio.spec.js（循环/单次、结束事件、偏移恢复） | 待验证 | v0.1.0 基线 |
| audio.gain.v1 | crates/nir-core/tests/audio_contract.rs（gain 为播放元数据+范围门） | 同文件（取消观测淡出提交设备值） | tests/nir-next/audio.spec.js（事件 gain×总线音量） | 待验证 | 事件音量与总线相乘，不预乘 PCM |
| audio.stop.v1 | crates/nir-core/tests/audio_contract.rs（淡出停止后取消、剩余段恢复） | 同文件（拒绝无效目标/伪造所有权） | tests/nir-next/audio.spec.js（跨存读档停止包络） | 待验证 | 淡出走故事时钟 |
| ui.image-menu.v1 | crates/nir-player/src/lib.rs（media_tests 缩放/悬停/回想解锁门） | crates/nir-player/src/replay.rs（冻结菜单页随回想恢复） | tests/nir-next/menu-elements.spec.js；menu-* 规格均经图片菜单页 | 待验证 | v0.1.0 基线 |
| text.visibility.v1 | crates/nir-player/tests/coordination.rs（临时隐藏不推进、不覆写脚本可见性） | 同测试（隐藏跨恢复/回退保持） | tests/nir-next/interface-hide.spec.js、tests/nir-next/reading.spec.js | 待验证 | 与 player.hide-policy.v1 分开声明 |
| text.voice-binding.v1 | crates/nir-core/tests/reading_contract.rs（绑定校验、失败关闭） | 同文件（未提交对白不能恢复伪造绑定） | tests/nir-next/sampled-reading.spec.js、tests/nir-next/reading.spec.js（并行语音等待） | 待验证 | 页首及页内 Gate 后绑定 |
| text.voice-timer.v1 | crates/nir-player/tests/coordination.rs（采样余量/静音冻结）；crates/nir-core/tests/reading_contract.rs（已知时长门） | crates/nir-core/tests/reading_contract.rs（恢复保策略、不能绕过能力检查） | tests/nir-next/sampled-reading.spec.js（Auto 等于采样余量+固定延迟） | 待验证 | 批次 29 |
| player.hide-policy.v1 | crates/nir-player/tests/coordination.rs（显式暂停只释放自己的 owner） | 同文件（策略须能力、遮罩不跨新会话） | tests/nir-next/interface-hide.spec.js（音频设备挂起对比默认政策） | 待验证 | 默认政策仅隐藏呈现 |
| player.auto-delay-policy.v1 | crates/nir-player/tests/coordination.rs（固定/零延迟显式、随暂停停表） | crates/nir-player/src/replay.rs（auto 随冻结会话恢复） | tests/nir-next/sampled-reading.spec.js（+0.5 秒固定等待） | 待验证 | 批次 29 |
| text.shadow.v1 | crates/nir-presentation/src/lib.rs（scene_tests 仅绘制不改阅读几何）；crates/nir-core/tests/tween_contract.rs（有界值门） | 无运行态（仅校验）：随对白外观快照恢复（tween_contract.rs 对白通道） | tests/nir-next/shadow.spec.js（像素+随隐藏关闭） | 待验证 | 单层有限偏移阴影 |
| stage.wipe.v1 | crates/nir-format/src/transition.rs（方向/软边有界）；crates/nir-core/tests/tween_contract.rs | crates/nir-core/tests/tween_contract.rs（冻结样式+进度、伪造被拒） | tests/nir-next/wipe.spec.js（边缘像素、暂停、存读档进度） | 待验证 | 保留旧 StagePresent 生命周期 |
| stage.mask.v1 | crates/nir-core/tests/tween_contract.rs（遮罩资产校验/准备/恢复入转场） | 同文件；crates/nir-player/tests/coordination.rs（遮罩离场资产集重入） | tests/nir-next/mask.spec.js（阈值像素/极性/存读档） | 待验证 | 消息/UI 根仍待实现 |
| ui.menu-elements.v1 | crates/nir-player/tests/menu_elements.rs（共享变换/裁切/绘制序） | crates/nir-player/src/replay.rs（菜单页/媒体冻结恢复） | tests/nir-next/menu-elements.spec.js | 待验证 | 语义见 MENU-ELEMENTS-SEMANTICS.md |
| ui.menu-state.v1 | crates/nir-player/src/menu.rs（局部动作原子/旧版本拒绝） | 同文件（覆盖层保留局部值、重入失效） | tests/nir-next/menu-state.spec.js | 待验证 | 语义见 MENU-STATE-SEMANTICS.md |
| ui.menu-services.v1 | crates/nir-player/src/menu.rs（只备活动媒体/偏好走既有服务） | 同文件（menu_peek 保留页面状态至恢复） | tests/nir-next/menu-services.spec.js | 待验证 | 语义见 MENU-SERVICES-SEMANTICS.md |
| ui.menu-navigation.v1 | crates/nir-player/src/menu.rs（父页局部值/旧输入拒绝） | 同文件（导航链过临时覆盖、新会话丢弃） | tests/nir-next/menu-navigation.spec.js | 待验证 | 批次 46；最多八层 |
| ui.menu-chrome.v1 | crates/nir-player/src/menu.rs（关闭自动导航保留作者控件/可达性） | 无运行态（仅校验）：crates/nir-core/src/validate.rs | tests/nir-next/history-flow.spec.js（builtin_navigation=false 页实际运行） | 待验证 | 批次 47；Web 无按钮缺失直接断言 |
| ui.menu-history-availability.v1 | crates/nir-player/src/menu.rs（只读事实、导航前复查） | 无运行态（仅校验）：只读事实随 revision 失效 | tests/nir-next/menu-navigation.spec.js（history_available 显隐） | 待验证 | 批次 47 |
| ui.menu-reading.v1 | crates/nir-player/src/menu.rs（恢复一次不推进、Peek 不代选） | 同文件（peek 保留页面状态/暂停/阅读模式） | tests/nir-next/menu-reading.spec.js | 待验证 | 批次 33；需 ui.menu-services.v1 |
| ui.menu-story.v1 | crates/nir-player/src/menu.rs（显式纯投影、派发前复查） | 同测试（Core::restore 往返） | tests/nir-next/menu-story.spec.js | 待验证 | 批次 40；只读导出 |
| ui.menu-stack.v1 | crates/nir-player/tests/menu_elements.rs（共享变换重排/稳定控件 id） | 无运行态（仅校验）：布局声明式 | tests/nir-next/menu-flow.spec.js、tests/nir-next/menu-story.spec.js | 待验证 | 批次 35 |
| ui.menu-text-button.v1 | crates/nir-player/tests/menu_elements.rs（标签/绘制/悬停/命中同一身份） | 无运行态（仅校验）：crates/nir-core/tests/streaming.rs（运行时校验） | tests/nir-next/menu-text.spec.js | 待验证 | 批次 36 |
| ui.menu-storage.v1 | crates/nir-player/src/menu.rs（空槽保存/覆盖确认一次性令牌） | 同文件（确认随版本回退过期）；crates/nir-player/tests/coordination.rs（底层存读档服务） | tests/nir-next/menu-storage.spec.js | 待验证 | 语义见 STORAGE-SERVICE-SEMANTICS.md |
| ui.menu-history.v1 | crates/nir-player/src/menu.rs（千行有界窗口/模板校验门） | 无运行态（仅校验）：窗口偏移为页面态，经 Replay 冻结恢复 | tests/nir-next/menu-history.spec.js | 待验证 | 语义见 MENU-HISTORY-SEMANTICS.md |
| ui.menu-history-flow.v1 | crates/nir-player/src/menu.rs（单页快照/滚动版本门） | 同测试（历史快照复用+恢复守卫） | tests/nir-next/history-flow.spec.js | 待验证 | 批次 44 |
| ui.menu-history-scrollbar.v1 | crates/nir-player/src/menu.rs（拖动状态/输入权威/裁切序） | 无运行态（仅校验）：拖动瞬态，位置归流程窗口版本 | tests/nir-next/history-scrollbar.spec.js | 待验证 | 批次 45 |
| ui.menu-values.v1 | crates/nir-player/src/menu.rs（单次提交/过期拒绝/边界校验） | crates/nir-player/src/replay.rs（绑定局部值冻结恢复） | tests/nir-next/menu-values.spec.js | 待验证 | 语义见 MENU-VALUES-SEMANTICS.md |
| ui.menu-effects.v1 | crates/nir-player/src/menu.rs（进入效果等待准备、关闭延迟退出） | 无运行态（不入故事快照）：会话重置终止（同文件） | tests/nir-next/menu-effects.spec.js | 待验证 | 批次 48；瞬态不入快照 |
| ui.replay.v1 | crates/nir-player/src/replay.rs（冻结/嵌套拒绝/准入失败整事务作废） | 同文件（outcome/手动退出恢复冻结会话、设备丢失续备） | tests/nir-next/replay.spec.js | 待验证 | 批次 49；实包全路线见批次 54 认证 |
| task.compose.v1 | crates/nir-core/tests/compose_contract.rs（VM 等待时链前进、All 失败优先） | 同文件（链中途存读档只续播）；crates/nir-player/tests/coordination.rs（链中途回退不重播） | tests/nir-next/compose.spec.js | 待验证 | 批次 50；parallel_all 无浏览器直接断言 |
| story.typed-result.v1 | crates/nir-core/tests/typed_result_contract.rs（写声明值后分支/超时 default/取消不写） | 同文件（游标快照恢复、篡改拒绝）；crates/nir-player/tests/coordination.rs（存读档/回退） | tests/nir-next/typed-result.spec.js | 待验证 | 批次 51；批次 52 LiveNovel 复用同核心 |
| media.webp.v1 | crates/nir-compiler/src/optimize.rs（有损保 alpha/无损逐像素） | 无运行态（仅校验）：crates/nir-compiler/tests/project.rs（按容器裁剪 requires） | tests/nir-next/menu-elements.spec.js 等像素断言规格经 fixture 实际解码 | 待验证 | 无容器级直接断言，发行按对象出现声明 |
| media.mp3.v1 | crates/nir-compiler/src/optimize.rs（采样率×码率编码门）；crates/nir-format/src/lame.rs（gapless 标签解析） | 无运行态（仅校验）：crates/nir-compiler/tests/project.rs | tests/nir-next/sampled-reading.spec.js、tests/nir-next/audio.spec.js（经 fixture 解码播放） | 待验证 | 循环音频保持 WAV |
