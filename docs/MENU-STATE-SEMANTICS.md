# 菜单局部状态与纯绑定

`ui.menu-state.v1` 扩展 ImageMenu 的有限局部数据，复用 `ui.menu-elements.v1` 的图文、分组和控件。本能力只提供标题/图片菜单上的局部状态，不把这些数据当作剧情变量，不宣称通用服务页面或故事 Interact 已完成。

## 数据与绑定

`locals` 为最多 32 个具名声明：

- `bool`：布尔 initial。
- `int`：I32 initial/min/max，必须 `min <= initial <= max`。
- `enum`：字符串 initial 必须属于 values；values 为 1–32 个互异、非空、最多 256 UTF-8 字节的字符串。

局部名称非空且最多 128 字节。每个菜单实例从声明初值开始，不共享别的页面或 Core 的变量。

元素可声明 `visible_when` 与 `enabled_when`，各最多 16 条条件，缺省为空且结果为真。数组做逻辑与，基础条件为 `local {name,equals}` 或 `profile {key,present}`；局部比较静态检查类型与域，Profile 为只读事实集合。父容器的显隐和启用状态传递给整个子树；隐藏元素不绘制、不命中、不参与焦点；禁用控件保留命中并阻止穿透。

可选 `ui.menu-reading.v1` 还允许 `reading_available {mode,available}`，读取 Player 当前允许的 auto / skip_read / peek_story 服务集合。`available = false` 表示该服务当前不可用，不授予执行权限。这不是剧情变量或源引擎对象存在性别名；来源转换必须独立证明对应关系。条件用于任意元素及其子树，即使没有 reading 动作也必须声明 reading/services 能力。变化进入菜单 revision，提交控制动作和值时都复查；加载、其他暂停者等会使可用性变化，不应将它误作固定的已读标签。

Text 元素可以用 `text_local` 引用一个局部值作为完整正文。整数输出十进制，布尔输出 true/false，枚举输出当前字符串；沿用原 Text 的字体、字号、颜色、换行和裁切。枚举的全部候选以及控件语义标签进入作者字体字集，不能只为当前值生成子集。没有动态拼接、任意函数调用、IO 或求值副作用。

## 动作与身份

ImageMenuAction 增加 `set_local {local,value}`，一次点击只赋值一个字段。赋值目标与常量由发行内容声明，宿主输入只携带控件身份，不能从点击消息任意指定值。

播放器给每个已投影的新元素控件生成 `MenuControl {instance,revision,control}`；使用局部状态的旧 buttons 也使用同一路由。instance/revision 是引擎生成的输入凭据，不是作者需要管理的 NIR 字段。执行前检查当前页面、加载状态、实例、版本、控件存在性、祖先条件和 Profile guard，然后重新验证赋值类型/范围。合法动作先消费 revision，再执行一次有限赋值或既有菜单服务请求；相同值的赋值也消费输入版本。

旧 revision、旧 instance、未知/隐藏/禁用控件只被丢弃，不修改局部状态或 Core，不降级成背景点击。动态菜单的具名页面/入口必须经过控件路由，不能用裸菜单/函数动作绕过条件；播放器自身原有 NewGame/Settings 等通用动作保持原本权限。局部状态和 Profile 会刷新 revision；可选 `ui.menu-services.v1` 也把 Preferences 纳入同一版本校验，SaveSlots/请求身份仍待实施。

## 生命周期与焦点

- 打开另一图片菜单、显式重新打开同名菜单或开始新的玩家 session：新 instance，locals 重置为声明初值。
- 临时打开播放器设置/存读档等覆盖页再关闭：locals 保留，但离开与返回都换 instance，旧输入不能重新生效。
- Profile 变化：刷新 revision，重新求值。
- 设备资源重建：不因 GPU 资源重建重放动作或重置 locals；准备期间不接受菜单控件点击。

控件编号按完整声明位置分配，不随兄弟显隐而改变。键盘焦点可跨 revision 留在同一 instance/control；实际激活取最新投影动作并重新校验 revision。页面实例不同或控件隐藏/禁用则清理焦点。Web 与共享原生焦点实现同一规则。

instance/revision 采用 checked u32，耗尽时报错，不回绕。标题菜单状态为瞬态，不写入剧情快照；未来故事内 View 状态须在 P5 中另行定义恢复契约。

## 中性例子

```json
{
  "background": "menu.background",
  "buttons": [],
  "locals": {
    "tab": {"type":"enum","initial":"Settings","values":["Settings","Saves","History"]},
    "slot": {"type":"int","initial":0,"min":0,"max":2}
  },
  "elements": [
    {"id":"tab.saves","rect":[20,20,180,50],"content":{"type":"hit_region","label":"Saves tab","action":{"type":"set_local","local":"tab","value":"Saves"}}},
    {"id":"heading","rect":[220,20,400,60],"text_local":"tab","content":{"type":"text","text":"Settings","size":32,"color":[1,1,1,1]}},
    {"id":"save.panel","rect":[20,100,0,0],"visible_when":[{"type":"local","name":"tab","equals":"Saves"}],"content":{"type":"group"}},
    {"id":"slot.two","parent":"save.panel","rect":[0,0,180,60],"content":{"type":"hit_region","label":"Select slot 2","action":{"type":"set_local","local":"slot","value":2}}}
  ]
}
```

该例只选择局部槽位，不执行保存。Preferences 的有限只读数值和服务动作见 [菜单服务](MENU-SERVICES-SEMANTICS.md)；params、SaveSlots/History 模型、故事显式导出、值控件、窗口集合与存储请求身份仍待交付，不能把这组页签演示算作 P3 的完整系统页验收。
