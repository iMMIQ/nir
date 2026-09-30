# 类型化属性动画

此文描述已实现的 `tween.target.v1`，不代表 NIR-NEXT 的 UI 页面、分域时钟或通用组合已完成。

## 格式与目标

```json
{
  "id": "message-fade",
  "scope": "session",
  "effect": {
    "type": "tween",
    "target": {"type": "dialogue_root", "property": "opacity"},
    "to": 0,
    "duration_us": "250000",
    "replace": true
  }
}
```

`scene_node` 接受 `node` 和 `property`；属性为 x/y/scale/opacity。`dialogue_root` 接受 opacity/background_opacity/text_opacity。消息属性均为有限 0–1，初始为 1；场景属性沿用 Clip 的范围。未知域、属性和字段拒绝解析。当前目标不包含 UI 页面、任意对象路径或音频总线；音频停止仍通过 AudioStop 绑定具体播放实例。

旧 `Clip` 保持原有编码，在执行与占用判断时归一化为 scene_node。同一 Cue 内对同一属性写入两次会拒绝，混用 Clip 与 Tween 也不能绕过。新格式需要显式声明 `tween.target.v1`；编译器仅在实际使用 Tween 时加入这项能力。任务数量和执行预算沿用 Core 现有限额。

## 执行与恢复

Tween 使用现有 Task、Scope、Await 和 Story 时钟，不产生第二控制流。激活提交时捕获可见值和基础值；参数由静态效果定义给出。没有 replace 时遇到活动 writer 失败；replace 时旧任务按其 CancelPolicy 结算，原因记为 Replaced，新任务从替换前的可见值开始。

Linear/Smooth、FinishPolicy 和 CancelPolicy 与 Clip 共用 ScalarTween。零时长在提交中完成。消息根身份不依赖场景代次：session 动画可以跨场景继续，scene 动画在场景退出时取消；frame/interaction 沿用现有作用域规则。新对白本身不重置 session 消息外观。

快照保存基础外观及任务捕获值、进度、策略。恢复检查外观范围、捕获范围、活动进度、当前场景节点存在性和同属性唯一 writer；不从头重播。菜单或后台暂停 Story 时，消息动画与现有故事动画一起冻结。

Source/runtime/content 版本不因这项可声明的新增能力整体升级。仍使用本轮尚未发行的 snapshot v2；此前发行的 v1 快照不迁移。新外观字段缺省为单位值，既有 DialogueVisibility 保持独立显隐掩码。

## 绘制

背景和边线 alpha 乘整体 opacity × background_opacity；正文、姓名、就绪提示 alpha 乘整体 opacity × text_opacity。长文翻页控件采用相同规则。主题原有颜色及消息框图片 opacity 继续相乘。其他场景、系统菜单、选项不受影响。

此处整体 opacity 是组件绘制 alpha 的共同乘数，不承诺隔离离屏分组混合。动画不改变排版、字素揭示、Gate 或点击语义；仅视觉透明不等于结束对白或解除 Gate。DialogueVisibility 仍保持旧行为，玩家临时隐藏的独立输入政策尚待 P2.2 实施。

## 验证边界

Core 契约测试覆盖独立通道、零时长、完成/取消、替换捕获、跨场景作用域、坏快照及能力/属性错误；旧 Clip 与 typed scene_node 使用两条公共路线对比 trace。Player 测试检查实际绘制 packet 的颜色相乘和场景不受影响。浏览器测试入口为 `playwright.nir-next.config.js`，包含画面像素变化及菜单暂停检查，实际运行结果记录在实施进展中。

LiveNovel 的 MESON/MESOFF 非零渐隐现映射为等时长的 dissolve 窗口揭示（`text.window-transition.v1`，见 [场景转场语义](STAGE-TRANSITION-SEMANTICS.md)），零渐隐保持立即翻转；源命令的等待/中断规则尚未认证，不能仅依据参数名称宣称已精确映射。Windows 真机与硬件 WebGPU 尚待实测。消息文字阴影、遮罩、动画指示器和 UI View 目标仍属后续交付。
