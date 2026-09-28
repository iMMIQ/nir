# 存档服务请求与候选事务

## 槽位读取身份

Player 为每次有效槽位读取分配单调 job，记录 slot、当前 session 和页面 instance。槽位仍为 0–2，不改变既有存储容量。

宿主 Load 命令包含 `{slot,job}`，成功回传 `slot_loaded {job,envelope}`，失败回传 `slot_load_failed {job,message}`。Web 的 IndexedDB 与 Windows 的存储适配均使用该协议。宿主不能把槽位读取降为无身份的 `loaded` 事件；后者目前仍用于文件导入及内部恢复测试。

Player 仅接收当前 job，复查 session 和页面实例，并检查 envelope.slot 与请求一致，再沿用摘要、快照及候选资源验证。接受一次后消费 job；重复回执不能再提交。新请求取代旧请求，返回或导航撤销旧请求；迟到成功和失败均不能更改当前剧情或显示过期错误。

没有改变磁盘槽键、SaveEnvelope 或快照格式。新的宿主与播放器必须使用同一 SDK 发行；不承诺将旧 host.js 单独替换进新发行。

## 候选准备期间返回

槽位回执通过校验后进入既有独占 Restore 候选通道。原 Core 在候选准备完成前仍是当前会话。返回、导航、新游戏、另一读档、导入或回退会取消该槽位候选及其非 locale 内容/媒体准备，清除候选和准备暂停；旧资源回执按请求身份丢弃。成功提交沿用现有原子切换及 session 更新。

取消只清除属于该次读档的诊断：槽位/摘要/快照校验、候选媒体、候选内容块及准入失败；无关宿主故障保留。内容块诊断携带 request 以判断所有权。候选媒体失败不再作为原 Core 的 PreparationFailed 输入；只有剧情 Activation 的媒体失败才通知该 Activation。

若原剧情在读档前已有待提交事件，返回后重新准备它并正常进入对白；无效存档尚未创建候选时，原加载请求保持有效，不取消后重建。独立 locale 事务不因取消候选而被取消。文件导入请求身份与通用槽模板仍需后续增量；三槽模型 revision 和覆盖确认见下节。本能力不作为 P3.3 全部完成的证据。

## 自定义槽位绑定（ui.menu-storage.v1）

`MenuSlot` 为 `{type:"fixed",slot:0}` 或 `{type:"local",name:"selected"}`。固定 slot 只能为 0–2；局部选择器只能引用上下界均位于 0–2 的 Int。源与 runtime 均验证，未知/无界/越界引用拒绝发行。

Text 元素的 `text_slot` 显示已有槽位标签，空槽使用作者声明的 `content.text`，与 text_local/text_preference 互斥。`save_slot {slot}`、`load_slot {slot}` 复用原存储服务，不直接操作宿主或剧情变量。标题上下文不能保存；忙碌槽不可重复保存或加载，空槽不可加载。投影禁用状态与服务执行检查一致。

槽位只保留三行。槽位元数据、版本、保存忙碌状态与读取请求状态变化都会使菜单 revision 更新；旧控制输入被消费且不能穿透。迟到列表不能降低已知已提交的槽位版本。

保存空槽直接创建原 SaveJob，仍使用 expected_revision。保存已有槽先打开播放器提供的确认页：底层菜单不保留命中/焦点节点；取消或 Escape 只关闭确认并回到原菜单，不保存、不离开剧情菜单。

确认令牌绑定发起控件、具体槽位和版本、页面 instance 与 session。确认前再次检查控件条件/权限和忙碌状态；版本变化、页面变化、取消、改为加载或标题退出使其失效。旧令牌和重复确认不能创建第二个保存任务。其他标签页并发写入仍由宿主 compare-and-swap 拒绝，确认不绕过存储冲突检查。版本耗尽返回 E_SAVE_LIMIT，不溢出。

确认目前使用内置系统页面，尚不提供作者自定义确认模板。正常 SaveJob 仍独立于页面：提交后离页不会取消已授权保存。旧内置 Save 动作保持原有行为；本能力的覆盖确认仅用于新 save_slot 服务。

```toml
[image_menus.system.locals.selected]
type = "int"
initial = 0
min = 0
max = 2

[[image_menus.system.elements]]
id = "slot.label"
rect = [80, 180, 600, 60]
text_slot = { type = "local", name = "selected" }
content = { type = "text", text = "没有存档", size = 30, color = [1,1,1,1] }

[[image_menus.system.elements]]
id = "slot.save"
rect = [80, 300, 240, 60]
content = { type = "hit_region", label = "保存所选槽位", action = { type = "save_slot", slot = { type = "local", name = "selected" } } }
```

这提供可执行的自定义选槽/保存/加载路径，不代表 P3.3 全部完成。通用集合子模板、Range/Toggle、作者确认布局、文件导入身份等继续独立交付。特定 History 只读窗口见 [历史窗口语义](MENU-HISTORY-SEMANTICS.md)。
