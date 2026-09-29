# 类型化交互结果

此文描述已实现的 `story.typed-result.v1`（Interact 类型化结果、取消路径与语义选择游标），不代表 NIR-NEXT 的故事内文本输入、IME 或通用编排已完成。

## 动机与格式

交互（Interact）此前只回答"走了哪条分支"；物品检查、密码面板这类交互需要把选择变成一个**值**写进剧情变量。类型化结果复用 Interact 控制流扩展两个字段，值始终由唯一 VM 写入，宿主只报告选项 id：

```json
{
  "type": "interact",
  "choice": "route",
  "branches": {"walk": "after_walk", "stay": "after_stay"},
  "on_empty": "failed",
  "result": "picked",
  "on_cancel": "gave_up"
}
```

选项在 choice 定义中携带常量值（类型必须与目标变量一致）：

```json
{"id": "stay", "text": "stay", "value": {"type": "i32", "value": 2}}
```

`result` 指定目标变量；`on_cancel` 声明显式取消路径，缺省表示交互是模态的（宿主不得提供取消出口）。两者任一出现即要求 `story.typed-result.v1` 能力；编译器按实际使用裁剪该能力。

## 提交语义

- **选择**：VM 校验选项存活且启用后，先把该选项声明值写入 `result` 目标变量，再走 `branches` 分支。宿主命名 id、从不提供值——`OfferedChoice.values` 只是呈现快照，提交以声明为准。
- **超时**：带 deadline/default 的交互超时按"显式选择 default 行"提交，同样先写 default 的声明值。
- **取消**：`CancelChoice` 跳转 `on_cancel`，**不写任何值**。它是一次完整输入：占用输入序列（`last_input`）、产生 Checkpoint、留下 `input:cancel` 痕迹；陈旧取消不能二次触发。
- **游标移动**：`SelectChoice` 是对挂起交互的**观察**——无输入身份、不推进 `last_input`、不产生检查点、序列号被忽略。它只移动语义选择游标 `OfferedChoice.selected`。未知选项、陈旧交互、禁用行一律忽略。

## 语义游标与瞬态边界

类型化交互携带语义选择游标（进入交互时为 default 行，缺省为首个启用行），它进入 Core 快照并随恢复返回；恢复后的游标仍是提交依据的呈现。悬停（hover）与键盘焦点是**呈现瞬态**，永不进入快照：引擎把落在选项行上的键盘焦点经普通动作路径同步为一次游标观察（`UiAction::SelectChoice`，序列 0），焦点离开或刷新不产生任何 Core 状态。

## 快照与恢复校验

恢复时对挂起交互执行权威校验，全部拒绝：

- `values` 必须与 choice 声明逐项相等（多出、缺失、篡改值均拒）；
- `selected` 必须命名当前存活且启用的行（类型化交互必须有游标）；
- `result` / `on_cancel` 必须与规范终结符一致（快照声称程序未声明的取消路径即拒）；
- 普通交互不得携带 values / selected / result 状态。

恢复的交互获得全新交互身份（含会话轮换），与既有恢复语义一致。

## 源与 Runtime 校验

源与 Runtime 两个站点共用检查：能力门控（E_CAPABILITY）、目标变量存在（E_VARIABLE）、每个选项都带值且类型匹配目标（E_TYPE）、`on_cancel` 必须命名同函数块（E_BLOCK）。

## 无限等待诊断（E_INFINITE_WAIT 推广）

定义的效果树内**任何位置**出现 `looped: true` 的音频叶子，都会使该定义的自然 Finished 里程碑不可达：序列停在该叶子、ParallelAll 永远等不齐全部子项。校验在包级与源级两个站点对整棵效果树扫描循环音频——直接 Await 循环音频、Await 包含循环子项的 sequence、Await parallel_all 内的循环子项，一律 E_INFINITE_WAIT；非循环音频保持可等待。

## 宿主集成

- `UiAction::SelectChoice { option }`：游标观察（引擎焦点同步与宿主直发共用）。
- `UiAction::CancelChoice`：仅当挂起交互自身声明 `on_cancel` 时派发；呈现层据同一状态渲染取消出口（Web 语义按钮、桌面 Escape），Web host 的 Escape 在可取消交互上优先取消，紧凑状态暴露 `choice_cancellable`。
- 键盘 Tab/方向导航落在选项行上时引擎自动同步游标（见上）。

## 测试

- Core 契约 8 项（typed_result_contract.rs）：提交先写值后分支、普通交互不携带结果状态、取消无写入且陈旧取消拒绝、游标移动/快照恢复无进度、超时提交 default 值、恢复校验七类篡改、源校验五类拒绝、循环音频三形态 E_INFINITE_WAIT（含非循环反例）。
- Player 协调 3 项（coordination.rs）：交互中途存读档恢复挂起交互与游标、类型化提交后回退撤销写入并重新挂起、取消分支无写入且无声明路径时拒绝。
- 浏览器验收 4 项（typed-result.spec.js，端口 4223）：选项声明值驱动不同对白、键盘焦点移动语义游标且不推进剧情、Escape 经声明路径取消且无写入、交互中途存读档恢复游标后照常提交。
