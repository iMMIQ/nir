# 有限菜单元素语义

`ui.menu-elements.v1` 在现有 `ImageMenu` 增加可选 `elements`，缺省为空。此阶段只扩展静态组合，不声明已经支持通用 View、局部变量、值绑定、系统服务模型或 Replay 隔离。旧背景和按钮格式继续有效，后续能力不能改变其缺省表现。

## 结构与顺序

元素由唯一 `id`、可选 `parent`、`rect`、正 `scale`、`opacity`、可选局部 `clip` 和带类型 `content` 构成。类型为 Group、Image、Text、Button、HitRegion。父节点只能是同一菜单中的 Group，禁止环；根与同级按声明顺序绘制，Group 的整个子树连续绘制。父节点可以晚于子节点声明。

背景先画，旧 buttons 其次，然后画 elements，播放器菜单/退出等覆盖控件最后画。元素中的图片与文字严格交错：晚画的半透明图片正常混合覆盖早画的文字，不将所有文字统一提升到顶层。Image 和 Text 不拦截输入；需要拦截的区域须显式提供 Button/HitRegion。

坐标使用作品 stage 空间，整体保持宽高比居中；子节点位置相对父节点原点，子 scale 不缩放自身位置。继承缩放、透明度和祖先裁切，裁切为交集。Group 只提供变换，不产生矩形绘制或点击。Text 使用作品正文语言、字体计划和玩家字体比例，在指定矩形与裁切内排版，不自动改变固定布局。静态文本仍是作者字面量，没有翻译 key；可选局部值绑定由独立 `ui.menu-state.v1` 提供。

## 输入与身份

Button 可选择普通/hover/locked 图片；没有 locked 图片时降低 RGB 亮度。HitRegion 不产生图片，但参与命中、键盘焦点和辅助语义。两者沿用既有有限 ImageMenuAction 与 Profile guard，不授予任意脚本或剧情变量写权限。

视觉、命中、焦点使用相同变换后的矩形与裁切。最上层命中控件优先，禁用控件消费命中且不向下穿透。透明度只影响绘制，不自动关闭命中。hover 使用实际命中控件身份，不用动作相等反推按钮。元素标识与旧按钮标识在单个菜单内共同唯一。新元素动作现在由播放器生成实例/版本/控件身份并校验，见 [菜单局部状态](MENU-STATE-SEMANTICS.md)；完整系统服务 revision/请求契约仍待实施。

## 有界性与兼容

单菜单 elements 与旧 buttons 合计最多 256 项，其中 Text 最多 64 项，含自身的层级最多 8 层。元素 ID 最长 128 字节，文字最长 4096 字节，控件标签最长 1024 字节。几何分量绝对值不超过 8192，宽高非负，非 Group 宽高须为正；缩放为 0.01–8，累积作者缩放为 0.001–16；opacity、颜色分量为 0–1；字号为 8–128。数值均须有限。

渲染器为有序文字提供最多 64 个独立 glyph 批次，共用字体缓存和 atlas，图片仍共用既有纹理及上传预算，不新增离屏合成目标。减少元素或离开菜单时回收多余文字批次。

源/runtime 均拒绝未声明能力的非空 elements；编译器按实际使用声明。所有状态图片进入发行与字体/资源准备闭包，入口继续校验无参数/无返回约束。配置为空不增加 capability，不改变存档结构或格式版本；旧播放器遇到新 capability 应明确拒绝，不静默丢弃菜单结构。

## 中性例子

```json
{
  "background": "menu.background",
  "buttons": [],
  "elements": [
    {"id":"panel","rect":[100,80,0,0],"clip":[0,0,500,300],"content":{"type":"group"}},
    {"id":"label","parent":"panel","rect":[20,20,400,60],"content":{"type":"text","text":"Settings","size":32,"color":[1,1,1,1]}},
    {"id":"settings","parent":"panel","rect":[20,100,200,80],"content":{"type":"button","label":"Settings","asset":"menu.button","hover_asset":"menu.button.hover","action":{"type":"settings"}}}
  ]
}
```

静态组合不等于完整 P3.1：锚点和额外文字测量规则尚未扩展。P3.2 的有限局部状态/条件/文字绑定基础见 [菜单局部状态](MENU-STATE-SEMANTICS.md)；params、服务模型与 P3.3 值控件、集合窗口及服务事务仍待交付。来源自动转换与原版菜单逐像素对照也不能由中性样例替代。
