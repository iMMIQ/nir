# 自定义剧情菜单与偏好服务

`ui.menu-services.v1` 允许主题声明 `menu_overlay`，用一个具名 ImageMenu 接管播放器的 Menu 页面；缺省保留内置菜单。作者可以只提供 system 菜单而继续使用默认标题页。该能力与静态元素、局部状态组合，不新增剧情执行流。

## 页面与资源生命周期

打开 Menu 沿用现有菜单暂停与返回目标：从 Story 打开暂停 Story，关闭返回同一会话；从标题打开则关闭回标题。自定义菜单默认提供内置返回按钮，Escape/右键仍使用共享 Close 行为。

`ui.menu-chrome.v1` 允许单页声明 `builtin_navigation = false`，省略自动添加的 Menu／Close／Back／Exit 按钮，让原页面控件使用完整舞台。缺省为 true，序列化省略缺省值，旧页面行为保持不变。作者声明的返回控件、Escape／右键及准备失败后的内置返回出口继续有效。源／Runtime 校验要求该能力，编译器裁剪未使用声明；这是静态页面政策，不增加局部状态或脚本执行。

Menu 动作可在当前覆盖页内导航到其他具名菜单，不改变标题菜单位置；关闭后再次打开从声明的根菜单开始。覆盖页局部值不带回下一次打开，标题页局部值在临时覆盖期间保留。实例变化使旧输入失效。

需要保存父页并逐层返回时，使用独立 [菜单子页导航](MENU-NAVIGATION-SEMANTICS.md) 的 push_menu／back。共享 Close 在该导航链中返回一层，原 menu 替换动作本身不创建导航链。

覆盖页专用图片不因启动标题页而预加载；保留既有标题菜单可达路径的准备行为。剧情运行时只为当前覆盖页准备和留存背景、普通/hover/locked 图。导航或关闭释放离开的页面媒体，不为隐藏子页面增加常驻预算。准备沿用媒体目录、资源准入、字体预检和迟到回执规则，不提高旧预算。

若正在准备剧情资源，先保留该准备事务，再补齐当前菜单资源；菜单准备不会取消剧情激活。离开页面仅取消 Menu 自己的资源/目录准备及暂停标记。准备失败使用内置返回/重试控件；关闭只清除归属于该菜单请求的错误，保留剧情和其他服务状态。

为避免把尚未隔离的剧情入口伪装成 Replay，覆盖页及其 Menu 导航闭包禁止 Entry。返回标题使用 Title 服务；完整隔离 Replay 仍待 P4。

## 只读偏好与服务动作

Text 元素可指定 `text_preference`，读取以下一个字段并显示两位小数：

- `bgm_volume`、`voice_volume`、`sfx_volume`。
- `font_scale`、`text_speed`、`auto_wait_scale`。

它与 `text_local` 互斥，只能用于 Text，求值没有副作用。布局反复测量不会写设置。

新增有限菜单动作：

- `adjust_preference {field,delta}`：delta 必须有限且绝对值不超过 4；由既有偏好服务应用与夹取范围，并持久化。音量为 0–1、字号比例为 0.8–1.5、字速和等待比例为 0.25–4。
- `toggle_reduced_motion`：调用既有减少动态效果偏好服务。
- `close`：返回覆盖页的原上下文。

字号变更仍经过排版 generation 和准备流程，音量仍通过玩家 bus gain 生效。这里没有第二套设置存储，也没有直接写 Core 变量的权限。

新元素、使用服务的旧按钮以及覆盖页按钮均使用实例/revision/control 凭据。Preferences（含宿主加载或其他内置页面修改）与 Profile 变化会刷新 revision；执行前复查页面实例、当前版本、条件和加载状态。一次有效动作消费版本，重复旧点击不能再次加值。焦点可留在同一实例/控件上，但激活使用新动作版本。

## 主题片段

以下片段在既有主题 manifest 中加入根字段和菜单；资源 ID 由作者目录提供：

```toml
menu_overlay = "system"

[image_menus.system]
background = "menu.background"
buttons = []

[[image_menus.system.elements]]
id = "speed.value"
rect = [80, 180, 220, 70]
text_preference = "text_speed"
content = { type = "text", text = "1.00", size = 40, color = [1, 1, 1, 1] }

[[image_menus.system.elements]]
id = "speed.increase"
rect = [350, 180, 300, 70]
content = { type = "button", label = "Faster", asset = "menu.button", action = { type = "adjust_preference", field = "text_speed", delta = 0.25 } }
```

`ui.menu-services.v1` 本身不等于 P3.3 完成。自定义三槽选择、标签绑定、保存/加载与覆盖确认另由 [ui.menu-storage.v1](STORAGE-SERVICE-SEMANTICS.md) 提供。历史只读窗口另见 [ui.menu-history.v1](MENU-HISTORY-SEMANTICS.md)。Range/Toggle、通用集合子模板、params/故事只读导出仍待交付。原 Saves/Settings 菜单动作仍进入内置服务页面。

## 阅读菜单动作

`ui.menu-reading.v1` 在现有菜单服务能力之上增加 `{"type":"reading","mode":"auto"}`、`skip_read` 和 `peek_story`。它们只能通过带页面实例、revision、控件 ID 的菜单请求进入；不允许把不受身份约束的宿主开关当成菜单提交。

Auto / SkipRead 先复查当前可用性，再原子关闭菜单、释放该菜单的暂停并启用指定模式；重复选择 Auto 表示保持开启并开始新等待周期，不是反向切换。SkipRead 只接受当前 text_id/meaning_revision 已读；后续停止于未读文本、硬 Gate 或选择的规则沿用阅读器。菜单服务不额外注入手动 Advance；Auto 仍按正常计时策略运行，因此允许的零等待可能立即触发自动推进。

PeekStory 临时隐藏菜单和阅读界面，只显示当前剧情画面。它保持菜单实例、局部值、资源和菜单暂停，以及既有 Auto/Skip 状态；Core 时间不前进。恢复输入被消费并返回同一菜单，不关闭菜单或推进文字。它与 Story 中的 ToggleInterface 不同，后者恢复后仍在 Story，且使用作品 hide_policy。Peek 状态不写入 Core 快照，也不作为一个新场景或回想会话。

可用性由 Player 提供给共享投影，并在提交时复查。标题、结束、加载/故障、保存确认及存在其他暂停所有者时均拒绝；Auto/SkipRead 还要求有对白且没有选择。PeekStory 可以临时隐藏选择项，恢复时保留原选择及交互身份；可用性改变更新菜单 revision。Peek 期间所有菜单控件/值写入均拒绝，恢复后旧 revision 仍不可重放。Web 与原生使用同一语义；宿主报告的是当前显示的屏幕。

来源证据：已读取目标 LiveNovel 的选择回调，Auto/已读跳过会退出菜单，隐藏文字则等待恢复输入后继续原菜单。转换器已在显式不完整草稿中生成源菜单布局，并绑定经校验的 Auto／已读快进／隐藏文字及相应的已读、回想和服务条件；原组件遍历由受限来源配置识别为 PeekStory，不在运行时执行源循环。原隐藏过程中的渐变及焦点恢复细节尚未认证。
