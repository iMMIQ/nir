# 有限任务组合

此文描述已实现的 `task.compose.v1`（Sequence/ParallelAll），不代表 NIR-NEXT 的故事内局部状态、类型化交互结果或通用编排已完成。

## 动机与格式

Activate/Await 只有单一等待槽：主 VM 等待对白时无法同时等待“先位移、位移完成后再淡出”。组合把这条链表达为一个自主任务，在 VM 停驻于任意等待、选项或内容屏障时仍自行前进：

```json
{
  "id": "chain",
  "scope": "session",
  "effect": {
    "type": "sequence",
    "children": [
      {"id": "move",  "scope": "session", "effect": {"type": "tween", "target": {"type": "scene_node", "node": "background", "property": "x"}, "to": 100, "duration_us": "3000"}},
      {"id": "fade",  "scope": "session", "effect": {"type": "tween", "target": {"type": "scene_node", "node": "background", "property": "opacity"}, "to": 0.2, "duration_us": "2000"}}
    ]
  }
}
```

`parallel_all` 结构相同、语义为全部子项同时启动。`sequence` 逐个启动子项，前一个完成后才捕获并启动下一个。子项可为 Tween、Clip、Audio、AudioStop、Delay 或嵌套组合；StagePresent 与 Dialogue 不得作为子项（舞台转场和对白拥有全局唯一的根，不能被链内复用）。

## 执行语义

组合本身是一个普通 Task：沿用现有 Scope（frame/session/scene/interaction）、里程碑锁存（Started/Finished）、Await、TaskControl 与 Story 时钟，不产生第二控制流。激活提交时冻结效果树参数与全部有限资源闭包；子项经统一的 `commit_effect` 路径派生，与顶层效果获得完全相同的所有权检查、属性捕获、AudioStart 意图与句柄注册——只是派生发生在子项自己开始的时刻，因此序列后项捕获的是前项结束后的当前值。

- **结果归并**：失败 > 取消 > 完成。任一子项失败即失败整链；ParallelAll 的取消同理；全部子项完成才记 Finished。链失败时仍在运行的子项按取消结算，链被 Finish 控制时运行中的子项按各自 FinishPolicy 落到终值，未派生的子项永不执行。
- **完成副作用不回滚**：已结算的子项保持其结果（位移停在已到位置、音频已播完即播完）。
- **零时长与预算**：零时长子项在同一提交的追赶（chase）轮内连锁完成，但每次派生消耗一个执行预算单位——预算耗尽时链跨步续跑，`work_used` 可见每次派生；追赶不收敛（超过 2×MAX_TASKS 轮）显式 E_LIMIT 故障而非挂起。树有限、深度与叶子数有上限，无限零时长循环不可表达。
- **并发写入**：序列位置先后独占运行，链内可以改写前项写过的地址；ParallelAll 兄弟之间、以及链与 Cue 内其他并发效果之间不得写同一属性地址（E_OWNERSHIP）。
- **结束级联**：`end` 终结符与作用域退出照常结束所有运行中任务，组合及其子项一并按上述策略结算；故事必须先 Await 链的 Finished 才能安全 `end`。
- **剧情边界**：组合不执行 Assign/Random/Call 等剧情或服务操作，无 Runtime Worker，不提供条件分支或循环控制——需要逻辑时仍由主 VM 表达。

## 命名与寻址

整个 Cue 效果树内 ID 唯一（子项不得与顶层效果或其他子项重名，E_DUPLICATE）。子项 ID 进入运行时句柄表用于诊断与快照，但**不进入故事可寻址的名字索引**：Await、TaskControl、DialogueVoice/Continue 只接受顶层效果 ID，按子项名等待或控制直接 E_TASK。组合只能作为整体被等待、被 Finish/Cancel。AudioStop 子项的目标同样只接受同模块顶层音频任务；目标播放实例在子项启动时必须仍存在。

## 校验

源与 Runtime 根共用同一套组合校验：`task.compose.v1` 能力（编译器仅在实际使用组合时保留）、子项作用域必须继承组合的 scope（E_SCOPE）、禁用子项类型（E_COMPOSE）、并发写冲突（E_OWNERSHIP）、全树 ID 唯一（E_DUPLICATE）、嵌套深度 ≤ 8（E_LIMIT）、单 Cue 总叶子数 ≤ MAX_TASKS 256（E_LIMIT）、Tween/增益/停止子项各自的能力与目标检查（E_CAPABILITY/E_AUDIO_STOP/E_AUDIO_GAIN/E_ASSET_TYPE）。编译器的能力裁剪与媒体根/激活配方遍历全树（`effect_tree_any` / `collect_audio_assets`），嵌套音频资产正常进入准备闭包。

## 快照与恢复

快照记录组合的已派生子项列表（children）与游标（cursor，恒等于 children 长度）。恢复校验：游标与已派生数一致、每个已派生子项与其声明（ID、scope、效果）逐项匹配、序列至多一个运行中子项、已 Finished 的链没有任何待办子项；损坏即拒绝（E_SNAPSHOT）。链中途存档读档/回退后：已完成的音频子项不重启，运行中的子项从冻结进度继续，宿主对运行中音频只按保存的故事偏移重发一次 AudioStart（续播而非重播），Tween 的 elapsed/captured 原样恢复。回退到链中途的检查点同样只回到该次提交的合法状态。

## 验证边界

Core 契约测试 8 项：VM 等对白时链自主前进、零时长连锁与预算、子项失败保留已完成副作用、ParallelAll 归并与兄弟取消、Finish/Cancel 控制整链、链中途快照恢复不重播、源校验拒绝（能力/作用域/禁用类型/并发写/重名/深度/叶子/停止目标）、恢复校验拒绝损坏组合。Player 协调测试 2 项：并行链中途本地变量改变后存读档、以及链中途检查点回退，均断言恰好一次携带故事偏移的 AudioStart、Tween 冻结值保留、链恰完成一次、结局到达。浏览器用例在真实 Web 播放器验证同一动机样例：主 VM 等待对白揭示时，先淡面板再淡正文的序列链自行走完两段。入口 `playwright.nir-next.config.js`。

LiveNovel 来源命令到组合的自动映射尚未实施；不能据此宣称来源动画序列已等价迁移。通用编排、条件子项与故事内局部状态仍属后续批次。
