# Replay 事务

`ui.replay.v1` 为图片菜单提供显式声明的回想（Replay）入口：控件动作 `replay` 指向一个具名函数，点击后启动一个隔离事务——冻结原会话、准备候选、成功后切换为唯一活动 Core，结束后把冻结的原会话作为恢复候选重新验证并原样接回。失败在任何边界都保留当前活动会话与页面。

```toml
[[image_menus.system.elements]]
id = "replay"
rect = [80, 160, 420, 80]
content = { type = "button", label = "Replay arrival", asset = "menu.blue", action = {type = "replay", function = "replay"}, requires = "seen" }

[[image_menus.system.elements]]
id = "exit"
rect = [80, 280, 420, 80]
content = { type = "button", label = "Exit replay", asset = "menu.blue", action = {type = "exit_replay"} }
```

## 三相事务

事务有 entering / active / returning 三相（引擎 `state().replay` 暴露 `inactive`/`entering`/`active`/`returning`）：

- **entering**：点击通过实例/版本/守卫复查后，先冻结原会话描述（Core 快照、检查点、屏幕与返回屏、图片菜单面、菜单页与局部值/父链/挂起标题页、auto/skip），再创建候选 Core 并只在候选内推进其入口块。入口块只走到第一个激活或内容屏障（预算独立上限），剧情工作从不在候选内运行；其声音、痕迹与 Profile 意图在候选激活前不存在。冻结页继续拥有屏幕；原会话保持活动。
- **active**：候选媒体（与冻结会话联合准入，重叠资产不重复计费）准备完成后切换：会话自增并重置音频、检查点清零重记、菜单面关闭、auto/skip 复位。旧入口 `ImageMenuEntry` 的既有行为不变——它仍直接替换会话，不进入本事务。
- **returning**：回想函数 `end` 出现 outcome，或在活动相手动 `exit_replay`，开始返回：冻结会话在候选中重新验证/准备（Restore 候选），期间回想保持活动且可恢复；提交后原会话、检查点、菜单页在全新会话与菜单实例/版本下接回（解冻同时提升两者，冻结前的菜单输入全部过期），页面效果按新实例重放。返回失败保留回想活动，可重试。

回标题与 NewGame 放弃整个事务（entering 丢弃候选与冻结态；active 直接替换会话）。

## 隔离不变量

- **单一 Core**：任意时刻只有一个活动会话；候选只在屏障外推进。
- **Profile 隔离**：活动回想内的 `profile_merge` 只写入候选状态，宿主不收到 `PersistProfile`，玩家 Profile 不变。
- **保存政策**：回想活动（active/returning）期间 Save/Export 拒绝；存在任何回想事务（含 entering）时 Load/Import 拒绝；Rollback 仅在无事务或已进入 active 后允许——returning 中回退会覆盖返回候选。
- **嵌套限制**：活动回想内再次点击 replay 控件、以及保存/读取控件动作在派发点复查即拒绝（`resolve_menu_control` 与动作派发双重检查，过期投影无法绕过）；`exit_replay` 仅在活动相有效，其余时刻（含 returning 中重复退出）幂等无操作。
- **并发守卫**：已有准备、恢复候选、槽位读取或另一回想进行中时，新的 replay 点击静默忽略；槽位读取进行中不可能开始回想。

## 失败与恢复

- **候选缺资源**：entering 中的资源/准备失败保留冻结页与原会话，错误按普通准备恢复（Retry 重启候选自身的媒体准备；设备丢失经 DeviceReady 后同样以候选自身资源恢复，激活号取候选待定 Cue 的 id）。
- **准入失败**：入口媒体联合准入失败时整个事务即刻作废（候选与冻结态一起丢弃，原会话从未察觉），诊断不提供 Retry（重试已无可提交的候选），释放后需重新点击从头开始。
- **入口内容缺失**：入口块跨越的内容屏障按 `ReplayEntry` 内容用途获取，到达后继续推进候选；内容失败与资源失败同样保留冻结页。

## 资源与校验

冻结的原会话计入既有快照与内容预算，不把整作媒体永久 pin：候选激活时留存修剪释放不再交集的冻结资产，返回时重新准备。主题含 `replay`/`exit_replay` 动作的程序必须声明 `ui.replay.v1`（源与 Runtime 双重校验，`E_CAPABILITY`）；`replay` 动作与旧 `entry` 同受 `E_THEME_ENTRY` 规则约束。编译器按实际使用保留能力。锁定（`requires` 键）在投影与派发点复查：未解锁控件禁用，伪造的当前授权动作同样在守卫处死亡。

不包含：共享变量写回（回想内的赋值不落回原会话）、sleep/awake 类来源语义、整作自动回想迁移或来源系统菜单的自动 Replay 映射——这些须另有来源规则，不默认属于 Replay。
