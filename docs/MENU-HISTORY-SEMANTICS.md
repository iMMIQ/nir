# 自定义历史窗口

`ui.menu-history.v1` 在现有菜单组合中增加只读 HistoryWindow，使用 Player 已有历史记录；同时依赖菜单服务能力。没有增加剧情线程、历史语音重播或任意数据查询。

## 有界窗口与行模板

`history_window` 声明 offset_local、limit、row_height、size、color；复用元素 rect、Group 变换、opacity 和裁切。offset_local 必须是下界 0、上界不超过 999 的局部 Int；limit 为 1–16。一页所有静态 Text 与 HistoryWindow 声明行数之和最多 64。非法范围、未知局部值/窗口、非有限样式在源和 runtime 校验时拒绝。

offset 从最新一条往前计数，窗口内按原历史顺序显示。作者菜单夹取到可显示最后一个完整窗口的位置；少于 limit 时显示全部已有记录。每行模板显示“说话人 / 全文”，没有说话人则只显示全文。行高固定，文字在该行和窗口/祖先裁切交集中绘制；超长文本不会扩张窗口，也不会自动创建更多行。可变行高、展开详情及任意子控件模板不在此能力内。

每行保留记录冻结时的 locale、font plan 和全文，不翻译为当前语言、不重新读取旧章节正文。Player 只复制可见窗口的条目；隐藏窗口不生成行，普通 Story 不构建历史呈现列表。内置历史页也只生成原有 3 条可见记录，保留其原翻页边界和长文本滚动行为。

行 key 是当前历史序列位置，以 session 和页面 instance 为作用域。系统菜单暂停 Story，故该作用域内不会追加或驱逐历史；翻页不会重编号已有条目。读档/新会话或重开页面使作用域失效。此 key 不作为永久存档 ID，不允许跨会话行操作；目前行模板纯文本，没有行服务动作。

## 翻页服务

`history_page {window,delta}` 由实例化 MenuControl 请求。window 必须指向同一菜单的 HistoryWindow；delta 非零且绝对值不超过 16。动作只对该窗口的 offset_local 做夹取后赋值，不改变 Core、历史全文或全局内置历史偏移。

投影禁用达到边界或目标窗口隐藏的翻页按钮，执行时再次检查。请求沿用页面 instance/revision/control；旧输入不能重复翻页。局部赋值、普通页签条件和 Group 组合继续有效。

```toml
[image_menus.system.locals.history_offset]
type = "int"
initial = 0
min = 0
max = 999

[[image_menus.system.elements]]
id = "history.records"
rect = [80, 120, 1120, 360]
content = { type = "history_window", offset_local = "history_offset", limit = 3, row_height = 120, size = 24, color = [1,1,1,1] }

[[image_menus.system.elements]]
id = "history.older"
rect = [80, 540, 240, 60]
content = { type = "hit_region", label = "更早记录", action = { type = "history_page", window = "history.records", delta = 3 } }
```

这是特定 History 模型的有限文本模板，未完成通用集合子模板、参数化 View、Range/Toggle 或全部 P3.3。现有历史存储上限仍为 1000 条/4 MiB，不增加历史或内容资源预算。

## 来源迁移限制

已检查的来源历史菜单把 `CallHist` 的 count 设为消息框高度，初始 index 为 `@HistoryCount - 高度`；滚动条回调提供 index，滚轮调整值为字号乘三，且 `cut_break=0`。已注册 FormatHist 的配置使用排版高度单位，不能直接映射成 offset_local/limit。

普通导入保留参数并报告未映射；LiveNovel 草稿在来源校验后生成连续历史子页和原图部件，详见 [导入工具](IMPORT.md)。来源完整格式器、分页间隔和历史保留政策仍未等价迁移。现有 v1 固定行窗口保持原语义，不通过更换单位改变旧作品。

## 批次 43 的连续布局核心记录

`nir-presentation::history::HistoryLayout` 已提供连续历史布局的 CPU 核心，尚未接入菜单声明、Player 页面会话或宿主输入，不改变上述 v1 能力，也不是可用历史菜单的交付声明。

- 输入是不可变共享历史投影，沿用 1000 条／4 MiB 正文和说话人预算；记录 key 严格递增，保留冻结语言和字体计划。布局仅持有记录共享引用及每条高度／起点索引，字形缓冲由 TextEngine 的既有 LRU 管理。
- 使用实际字体和宽度排版全文。每次最多测量 16 条，通常至多 64 KiB；单条超过该字节数时允许单独整条测量，避免任意切割破坏双向文字、字形和换行。单条仍受整个历史预算约束，额外包含说话人分隔换行；这不是严格的帧耗时保证。尚需在宿主接入时验收超长记录响应性。
- 准备完成之前不提供滚动范围或接受滚动，缺字体或范围越界明确失败，不发布半成品范围。准备完成默认跟随最新端。滚动不改历史或剧情；重排按首个可见逻辑字符及行内比例恢复，最新端继续跟随最新端。
- 只为与视口及祖先裁切相交的记录生成 TextRun，首尾记录可部分可见，长记录不会按固定行高截断。单次最多 64 个可见文本 run，超出显式报错，不静默丢条目。几何总高度上限为 16,777,216 个布局单位；样式和裁切要求有限值。
- 当前核心的记录间 gap 与来源的 scenario-page 间隔不是同一语义；说话人另起一行也是当前布局模板，尚未认证为来源格式。来源的 `cut_break`、格式器、分组边界仍需在导入映射时明确，不能直接把保留的来源参数塞进这些字段。

后续还需：NIR 声明／能力校验，Player 按页面实例冻结和释放历史投影，Engine 分批准备与错误呈现，带实例和 revision 的滚动请求，滚动条／滚轮／键盘两宿主接入，原边框素材及回调映射，以及实际原包播放验证。

后续进展：连续窗口现已通过独立 `ui.menu-history-flow.v1` 接入 NIR 声明、Player 页面投影和 Engine 滚动输入；见 [连续历史窗口](MENU-HISTORY-FLOW-SEMANTICS.md)。上节保留批次 43 的历史状态；图片滚动条和 LiveNovel 原图子页草稿已有后续实现，完整来源分页语义仍待完成。
