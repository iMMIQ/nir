# 首版能力表

此表说明当前实现，附件中其他条款不会被默认为支持。未知指令、未知核心字段及非 `web-v1` 配置均拒绝加载。多模块契约与验证范围见 [模块工作流](MODULE-WORKFLOW.md)。

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

### media.webp.v1 / media.mp3.v1

打包媒体优化按实际发出的对象容器裁剪 `requires`：发行中存在 WebP 对象才声明 `media.webp.v1`，存在 MP3 对象才声明 `media.mp3.v1`；`--no-optimize` 或逐资产例外导致发行不含这两类对象时不声明，旧运行时仍可加载，播放器据此拒绝不支持的组合。默认构建把图像对象转有损 WebP（质量 92，alpha 通道在 ALPH 块中无损保留）、非循环音频转 MP3 CBR；尺寸、`duration_us` 和 `decoded_bytes` 描述符保持源资产值。MP3 对象带 LAME gapless 标签，原生加载器与浏览器 `decodeAudioData` 都按标签裁剪编码器延迟/填充，解码样本数与源 WAV 一致（原生加载器经测试逐样本对齐）。循环播放的音频保持 WAV（样本精确循环），MP3 不能表示的采样率、转换后不缩小的对象和字体不受影响；逐资产例外用 catalog `optimize` 字段，CLI 覆盖与转换缓存见 [编写与维护作品](AUTHORING.md)。
