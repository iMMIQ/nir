# 菜单页面效果

`ui.menu-effects.v1` 在已有图片菜单上增加声明式页面呈现效果：进入/关闭边界的一次性音效与有限渐隐、接受提交的点击音效，以及前台域的循环页面音乐。效果只在页面边界触发——准备完成、换页、接受提交、提交关闭。进入/关闭转场可在 `ui.menu-transition.v1` 下声明空间样式（wipe/mask），把页面整体分流到离屏页根做遮罩合成；进入边界的逐元素动画在 `ui.menu-element-tween.v1` 下声明，见下文。

```toml
[image_menus.system]
background = "menu.black"
buttons = []

[image_menus.system.effects]
click = "audio.bell"

[image_menus.system.effects.enter]
sound = "audio.bell"
fade_us = "400000"
style = {type = "wipe", direction = "left_to_right", softness = 0.2}

[image_menus.system.effects.close]
sound = "audio.bell"
fade_us = "700000"
style = {type = "wipe", direction = "right_to_left", softness = 0.2}

[image_menus.system.effects.music]
asset = "audio.bgm"
bus = "bgm"
gain = 1.0
```

## 触发边界与所有权

进入效果在页面就绪后触发一次，属主是 `(页面 ID, 菜单实例)`：覆盖页只在自身媒体准备完成（prepared stamp 完整）后触发，标题闭包页随启动准备就绪——Preparing 空窗从不发声。换页或离开菜单面（含未经关闭渐变的路径）立即停止旧页音乐并退休旧页效果；页面效果从不超过其页面。同一页面的局部状态修订（revision 变化）不重播进入效果。

点击音效是接受反馈：只在控件动作通过实例/版本/守卫校验并真正提交时播放一次，被拒绝或过期的请求、恢复（restore）驱动的投影重放都不触发。音效资产与音乐随页面图片一起进入准备集合（`prepared_assets`），启动/换页准备负责解码，页面留存（retention）在页面打开期间保持这些缓冲——准备完成的页面不会寻址未解码的音频缓冲。

## 关闭事务

声明的关闭效果把退出变成有限事务：关闭音效立即播放，旧页面输入随实例锁定，渐隐期间保持页面与暂停；淡出完成后才提交退出（返回 Story/标题、恢复阅读模式或取消的槽位恢复）。阅读模式关闭在提交时以原始交互身份重放开关动作。故障与紧急路径（恢复、会话重置、设备事件）可取消事务立即退出；进入故障保持标题静默。

## 空间揭示样式（ui.menu-transition.v1）

进入/关闭转场可声明 `style`，复用舞台 StageTransition 的 wipe（方向 + 软边）与 mask（图片遮罩 + 通道）语义。dissolve 或未声明样式保持旧的整层 alpha 渐隐路径，不声明也不需要 `ui.menu-transition.v1`；只有空间样式（wipe/mask）要求该能力，源与 Runtime 校验在「使用而无能力」时以 `E_CAPABILITY` 拒绝，编译器按实际使用裁剪声明。

空间揭示把页面从共享菜单面整体分流到离屏页根（页面背景、页面控件与页面文本随行），以样式覆盖度合成在冻结的底层帧上：进入时揭示覆盖底层帧，关闭时反向擦除露出底层帧；期间不叠加 alpha 渐隐，页面不透明度恒为 1。连续历史窗口在页面内拼接的历史滚动条等页面专属元素同样被吸收进页根，跟随揭示一起飞行。

渐隐（alpha 与空间样式同此）的收尾发生在纯时钟 tick 内，且与 ForegroundClockToken 的释放同刻——宿主帧循环随即停摆，周期性重投影的安全阀不会再触发。因此收尾本身必须标记视图脏：渐隐由有到无的那一 tick 产生一次性的「视觉脉冲」（`take_ui_visual_pulse`），引擎将其并入 state_dirty，保证落定帧被投影与提交。

mask 的遮罩是 Image 类页面资产：校验拒绝非图片资产（`E_THEME_ASSET`），并随页面图片进入主题闭包、准备与留存（`prepared_assets`），不从音效闭包取用。样式边界的渐隐时长仍须在 (0, 2 秒] 内（`E_VIEW_EFFECTS`）。

## 元素进入动画（ui.menu-element-tween.v1）

```toml
[[image_menus.system.effects.elements]]
element = "slide"
property = "offset_x"
from = -1400.0
delay_us = "0"
duration_us = "2000000"
```

进入边界可声明至多 128 条元素轨道：`element` 指向本页元素 ID（缺失以 `E_VIEW_EFFECTS` 拒绝），`property` 为 `opacity` / `scale` / `offset_x` / `offset_y`，`from` 为属性在动画首帧的值（opacity 0–1、scale 0–8、偏移 ±4096），`delay_us` 与 `duration_us` 沿页面边界起算（时长 (0, 2 秒]，延迟 ≤ 2 秒），同一元素的同一属性只允许一条轨道。

轨道随进入边界与页面渐隐共用同一前台时钟租约启动（边界已持令牌则复用，否则新租；无令牌可用时页面停在作者值而非卡在动画中），在 `delay_us` 内保持 `from`，随后线性/缓动推进到落定值：opacity 与 scale 落定到元素的作者值，偏移落定到 0——完成的轨道被丢弃，落定后的投影与从未声明动画不可区分。投影层在布局前把轨道值覆盖到元素的 Node（父级变换随之传播，透明度乘进颜色 alpha），页面本身仍走共享菜单面：不建立离屏页根，也不叠加页面级 alpha 渐隐。

逐页重启：换页清空在飞轨道并按新页声明重新启动；离开菜单面（含关闭事务提交、退休路径）清空轨道并释放时钟令牌，动画绝不越过其页面。最后一条轨道的落定 tick 与渐隐收尾同刻释放令牌，同样以一次性视觉脉冲标记视图脏。`reduced_motion` 抑制全部元素动画（音效与音乐不受影响）。状态瞬态、不入故事快照，会话重置随宿主域重置终止。使用而无能力在源与 Runtime 校验以 `E_CAPABILITY` 拒绝，编译器按实际使用裁剪声明；引擎状态面以 `menu_element_progress` 暴露最靠前轨道的归一化进度（无动画为 null）。

## 时间与音频域

效果状态属前台 UI 时间域：渐隐按前台时钟推进，需先取得 ForegroundClockToken（数量以 MAX_TASKS 为上限），完成或取消即释放；菜单暂停只停剧情时钟，不动前台时钟。一次性音效与页面音乐都在 `foreground_ui` 音频域，宿主按 `(域, 会话, 任务)` 定位；会话重置（新游戏、回标题）随宿主域重置终止全部效果声音，不产生显式 AudioStop。循环页面音乐从不进入任何等待完成集合。未知前台音频失败不构成故障。

`reduced_motion` 开启时保留音效与音乐，跳过所有渐隐与空间揭示（边界立即完成，不申请时钟令牌，页面留在共享菜单面），关闭事务立即提交退出。

## 资源与校验

效果状态是每菜单会话的瞬态，从不写入故事快照；源/Runtime 校验进入既有媒体闭包与页面预算。渐隐上限 2 秒（样式边界同样受限），音效/音乐资产 ID 最多 128 字节，音乐增益 0–4，`deny_unknown_fields` 拒绝未知字段（错误码 `E_VIEW_EFFECTS`）。编译器在实际声明效果时保留能力（空间样式连带 `ui.menu-transition.v1`，元素动画连带 `ui.menu-element-tween.v1`）并连带菜单服务语义；效果音频对象进入既有打包与准备预算，不新增媒体上限。旧页面、候选页与退出中页面共享既有菜单媒体预算。

不包含：任意补间/关键帧系统、消息根或消息窗口的渐隐（属 `text.window-transition.v1`）、恢复时的效果重放或效果等待语义（等待页面音乐结束）。来源 LiveNovel 菜单的选择音／回想 BGM／系统菜单进出渐隐（`--draft` 草稿页，整层渐隐近似）由导入账本映射为页面效果（见 [导入](IMPORT.md)），悬停音效与动画光标不在其内。
