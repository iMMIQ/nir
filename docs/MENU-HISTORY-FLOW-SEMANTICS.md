# 连续历史窗口

`ui.menu-history-flow.v1` 增加 `history_flow` 菜单元素，依赖 `ui.menu-elements.v1` 和 `ui.menu-services.v1`。它使用真实文字排版的连续范围，旧 `ui.menu-history.v1` 固定行／按条目翻页的语义保持不变。不增加剧情控制流或存档游标。

## 声明与预算

```toml
[[image_menus.system.elements]]
id = "records"
rect = [80, 120, 600, 360]
content = { type = "history_flow", size = 30, line_height = 45, gap = 18, wheel_step = 90, page_step = 180, max_visible = 32, color = [1,1,1,1] }
```

每页最多一个连续历史窗口。复用父级变换、裁切、透明度、显隐和启用条件。size 为 8–128，line_height 为 size–512，gap 为 0–1024，wheel_step/page_step 为 1–8192；均要求有限数值。max_visible 为 3–64，与静态文字、文字按钮、既有历史窗口声明行数共同计入每页 64 个文本 run 上限。

为覆盖最小读者字号比例 0.8 及首尾部分可见记录，声明高度必须满足 `height <= line_height * 0.8 * (max_visible - 2)`。实际绘制仍复查可见预算，不会通过丢弃记录掩盖超限。源 Program 与 Runtime 根都校验能力、几何和预算；编译器裁剪未使用的能力。

记录按历史顺序连续排版，默认定位到最新端。保留冻结的正文、说话人、语言和字体计划；说话人非空时另起一行显示正文。gap 是每条 NIR 历史记录之间的布局间距，末尾无多余间距。长记录保留全文，首尾由窗口及祖先裁切。选择状态使用当前 UI 字体，原选项标签仍使用原正文计划。当前无记录内富文本或任意模板。

## 可选语音控件

`history_flow` 与 `history_window` 都可在 content 中增加 `voice_controls = true`，要求独立能力 `ui.menu-history-voice.v1`。缺字段默认为 false，旧布局保持原样。编译器按使用推断能力，Source 与 Runtime 根都拒绝缺能力的声明。LiveNovel 导入不自动开启。

只有显式 DialogueVoice 绑定留下的语音记录提供重播按钮；选择记录和旧存档的纯文本记录不显示按钮。重播共享玩家 Voice 音量，按原 gain、原绑定顺序串行准备 MP3，使用独立的 ForegroundUi/Voice；不会推进剧情、写变量或加载旧章节执行体。页面只能有一条重播，准备时可停止，失败可就地重试。正文 BGM、原 Voice 实例及阅读位置沿用菜单暂停政策。

控件以最终视口单位布局：宽度至少 280 时，正文留出 126px，右侧按钮 118×44px；宽度 72–279 时，按钮在正文上方，预留 52px；不足 72 时只显示文字。固定行窗口开启控件后，行高至少为宽屏 52px／窄屏 76px，并按实际可见高度减少每页行数，保留当前页的最新记录；整页翻页命令跟随实际容量，逐条命令仍按原 delta。绘制、按钮启用和 Engine 提交使用相同容量，旧页的容量无效，最早与最新记录均可到达。连续窗口按真实正文高度加上控件前缀测量，重排保留正文或控件区域的锚点。仅在窗口、祖先 clip 和画布内完整显示的按钮参与语义命中；空间擦除或遮罩还在显现期间不接受重播。

按钮文字也计入 64 个 text run 预算。固定窗口开启时每声明行预留两个 run；连续窗口 `max_visible` 为 6–64，并要求 `height <= line_height * 0.8 * (floor(max_visible / 2) - 2)`，实际绘制还复查正文、选择标题与按钮总数。生成按钮计入菜单语义控件预算。过密布局显式拒绝。

重播请求包含菜单 instance/revision、窗口 ID、记录 key，连续窗口还包含 layout 版本。Engine 对照当前投影的可用按钮复查完整请求，再交给 Player；裸 Player action 无布局授权。播放后按钮变为停止，旧重播动作不能再次启动；键盘焦点仍跟随同一记录。滚动、固定窗口翻页、隐藏、禁用、关闭、读档、设备丢失与会话切换停止或取消重播，过期媒体及播放回调无效。重排或 clip 变化后，如果按钮不再完整可见，也停止重播。语音重播和滚动位置都不进入故事快照；恢复后重新打开页面并主动选择重播。

## 页面生命周期与排版

Player 在页面实例建立时冻结共享历史投影，沿用 1000 条／4 MiB 正文和说话人预算。反复构造 UiModel 只共享快照，不复制所有正文；关闭、切页或新会话释放该页持有的投影。隐藏的窗口不绘制或接受输入，同一实例的显隐可保留已测量位置。系统菜单仍暂停 Story，历史操作不写 Core、Profile 或存档。

Engine 分批测量，每次最多 16 条，通常最多 64 KiB；单条超出该字节数时单独整条测量，仍受总历史预算约束，额外包含说话人分隔换行。该工作上限不是严格帧耗时保证。准备过程中显示加载提示，UI 可继续取得调度，Story 保持暂停；只在测量完成后发布滚动范围。错误显示可返回的提示，不发布部分范围，具体诊断保留在引擎状态。

只生成视口内相交记录的文本 run，字形缓存复用原有 LRU。首次进入跟随最新端；字体或视口变化重排时，保留首个可见逻辑字符及行内比例，处于最新端时继续跟随。位置仅在页面内有效，不保存到故事快照。

## 输入

连续滚动使用专用 `menu_history_scroll` 请求，携带页面 instance、菜单 revision、窗口 ID 和布局 layout 版本。step/page 只接受 delta 为 -1 或 1；position 只接受有限的 0–1 比例。Engine 在提交前按当前投影复查，准备中、隐藏、禁用、加载中及旧页面／旧布局请求无效，不穿透到故事。每次接受滚动或重排都会更新布局版本。

Web 滚轮和窗口内的垂直拖动使用 wheel_step；PageUp/PageDown 使用 page_step。原生端滚轮按指针命中的窗口处理，PageUp/PageDown 使用当前滚动窗口。wheel_step 随舞台及读者字号比例缩放，page_step 随舞台缩放。Windows 目标已通过交叉编译，设备操作仍待验收。

Web 数值键按当前语义控件 ID 及页面动作身份解析，复查当前值和布局，不依赖排队的焦点通知先完成。DOM 焦点和 Home 在同一轮到达仍能处理；数字 ID 被新页面复用时不能授权旧控件。网页发行存档入口只在菜单根页显示，避免覆盖作者子页的边缘控件。

作者图片滚动条使用独立能力，见 [图片历史滚动条](MENU-HISTORY-SCROLLBAR-SEMANTICS.md)。LiveNovel 草稿可以在来源契约校验后生成原图历史页，限制见 [导入工具](IMPORT.md)。来源 `CallHist` 的 index/count/cut_break、格式器和 scenario-page 间隔不能直接等同于上述字段；生成草稿不代表原历史页完整兼容。

## 只读历史可用性

`ui.menu-history-availability.v1` 增加显隐／启用条件 `history_available {available: bool}`，依赖 `ui.menu-elements.v1`、`ui.menu-state.v1` 和 `ui.menu-services.v1`。它只查询当前 Core 是否有历史记录，不冻结或复制正文，也不要求声明历史窗口。`available=false` 表示无记录时满足。

绘制、Stack 排布、命中和提交前校验读取同一事实。事实改变刷新菜单 revision；即使缓存尚未刷新，动作提交也复查当前 Core，因此恢复或会话切换不能让旧按钮权限继续有效。条件不会写历史、剧情变量、Profile 或快照。源／Runtime 校验要求独立能力，编译器裁剪未使用声明。

```toml
[[image_menus.system.elements]]
id = "open-history"
rect = [80,120,500,60]
visible_when = [{type = "history_available", available = true}]
content = {type = "hit_region", label = "历史", action = {type = "push_menu", menu = "history"}}
```

示例中的 push 另需 `ui.menu-navigation.v1`。来源的“消息框存在且历史像素范围非零”还需独立映射消息框可用性，不能仅凭记录非空认定来源条件等价。


菜单页资源按实际访问准备。启动只加载当前标题页及它的效果音和音乐，不递归准备所有可达子页。Menu、PushMenu、Back 和逐层 Close 均先准备目标页，期间继续呈现已提交的原页，保留它的音乐、locals 和父链；原页控件禁用，显示“取消切换”。图片、音效和音乐全部准备成功后原子提交目标页，才退休旧页音乐并触发新页 enter。失败允许重试或取消；取消保留原页，迟到回执不能切页。父链只保存菜单名称和有界 locals，不固定隐藏页媒体。设备重建期间取消目标时只重建原页；重建前的回执不能发布目标。
