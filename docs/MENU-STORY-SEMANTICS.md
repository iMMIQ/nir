# 菜单中的故事只读条件

`ui.menu-story.v1` 为 ImageMenu 增加显式的 `story_exports` 别名表。每页最多 32 项，别名和变量名各最多 128 字节；目标必须是已声明的 bool 或 i32 剧情变量。字符串、任意对象、宿主内存、存档字节不在该契约内。

```toml
[image_menus.system]
background = "menu.background"
buttons = []
story_exports = { replay = "story.replay_active" }

[[image_menus.system.elements]]
id = "save"
rect = [100, 100, 400, 60]
visible_when = [{type = "story", name = "replay", equals = false}]
content = {type = "hit_region", label = "Save", action = {type = "saves"}}
```

`story {name, equals}` 可用于 `visible_when` 或 `enabled_when`，比较类型必须与目标变量一致。源工程和 Runtime 根均复查变量存在、类型、声明和能力；缺失值求值为不匹配，包括比较 false 时。页面只能读取显式别名，不能通过别名访问其他 VM 数据。

该投影没有自己的存储：值来自当前 Core 故事状态，按现有 Assign／存档恢复规则变化。菜单打开、绘制、布局、焦点和条件测量均不写剧情，不增加另一套控制流。SetLocal／值控件只作用于已有菜单局部数据；同名局部值不会获得故事写权限。

Player 为当前页面提供有限投影。投影改变时刷新菜单 revision；动作和控件值提交前从当前 Core 再求值。旧 revision 不可重放，即使条件后来又恢复成立。隐藏元素的文字、语义、命中与 Stack 排列共享同一判断。页面切换及故事会话恢复仍遵守原实例失效规则。

此能力不等于任意表达式系统，也不表示 Replay 已实现隔离。转换器可以将已识别的来源上下文映射为剧情标量，在入口写入，再显式导出给界面。例如当前 LiveNovel 草稿将主线／回想包装入口映射为 bool，用于禁止回想中的已读快进；完整回想服务仍属于 P4。

格式版本不整体升级；字段缺省为空，旧菜单保持原行为。新增能力由编译器按实际导出／条件保留。Core 快照仍只保存原剧情变量，不保存只读投影副本。
