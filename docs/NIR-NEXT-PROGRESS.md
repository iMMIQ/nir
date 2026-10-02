# NIR-NEXT 实施进展

本记录区分实施计划与实际交付，不把本轮增量视为整套 NIR-NEXT 已完成。

## 当前代码增量

- 保留 Running/Finished/Cancelled/Failed，增加任务终态原因：正常完成、媒体自然结束、显式完成/取消、替换、scope 退出和失败。Await 分支优先级不变，迟到事件不改写已有终态。
- Audio 增加可选 `gain`，缺省 1，范围为有限的 0–4。通过 CoreIntent/AppCommand 传到 Web/Windows；新播放及恢复都保留事件音量。用户修改总线音量后仍与事件 gain 相乘。
- 源 Program 与惰性模块验证器检查 gain 范围和 `audio.gain.v1` 声明。编译器仅在实际使用非单位 gain 时声明该新增能力；旧能力集合尚未改为完整推导。
- 导入器保留单位增益媒体，通过事件参数表达不同音量；同一音频不同事件 gain 复用媒体配方，不再将放大烘焙进 PCM。
- Web 音频完成交付再次检查当前实例，防止旧播放回调影响替换后的播放。

## 版本选择

Source Program 1、runtime 2、内容包版本保持不变。新增 gain 缺省为 1，单位值序列化省略；非单位值必须声明能力，旧运行时会拒绝未知能力/字段，不能静默忽略。

Snapshot 升为 2，因为持久化任务新增终态原因以及带 gain 的效果定义；恢复检查状态/原因一致性，Running 不允许终态原因。旧发行继续由原配套运行时打开存档；新运行时明确拒绝 v1 快照，不提供跨发行存档迁移。

SDK 与 CLI 必须配套重新构建/resolve；源树修改不自动升级已经发布的游戏。

## 尚未完成

| 计划 | 状态 |
| --- | --- |
| P0 来源基线、三例、完整版本/限额矩阵 | 部分：已有代码盘点和回归基线；第二来源认证与三个完整新样例未完成 |
| P1.1 播放实例和终态原因 | 已有实例协议上补原因及验证；跨设备实测继续验收 |
| P1.2 类型化目标 | 计划所列四个域均已接入共享求值：场景节点与消息根（Core 故事轨道），AudioInstance 实例增益补间（`audio.gain-tween.v1`，Core 轨道）与 ViewElement 菜单元素动画（`ui.menu-element-tween.v1`，Player 瞬态）于批次 58 交付；跨设备实测继续验收 |
| P1.3 分域时钟/暂停 | Story/Foreground UI 逻辑时钟、独立暂停和宿主音频路由基础已实施；页面 owner 与关闭效果尚待 P3/P4 接入 |
| P2.1 事件增益、淡出停止 | 两项代码已贯通；Core/采样包络/WebGL2 回归通过，Windows 真机待验收 |
| P2.2 阅读边界 | 语音绑定、Auto、字速/等待偏好、隐藏、held skip 与设备包络检查点已实现并有 Web 回归；原版全路线与硬件认证待完成 |
| P2.3 消息框 | 消息根轨道、阴影与阅读提示已接入；来源样式/字体及完整源映射仍待完成 |
| P2.4 遮罩 | 方向 wipe 与纹理 mask 已有恢复及软件 WebGL2 验证；消息根窗口揭示（dissolve/空间样式、时钟冻结、存读档续播）已交付（批次 56）；UI 菜单页根空间揭示（MenuTransition 样式、ui.menu-transition.v1）已交付（批次 57）；来源映射与硬件验证仍待完成 |
| P3 页面组合与服务 | 静态组合、有限局部状态、故事只读条件、Stack／文字按钮、Range／Toggle、偏好和存读档绑定、确认令牌与固定／连续历史已实现；图片滚动条及有界子页返回已完成 Web 验收。原系统菜单已有三个阅读动作及历史页草稿自动迁移，历史格式器／分页间隔／保留规则仍有差异；完整来源系统页、通用集合与服务覆盖仍待完成 |
| P4 页面效果 | 菜单页效果与预置效果音频已实现（批次 49）；消息根转场已交付（批次 56）；菜单页面根空间揭示转场已交付（批次 57）；进入边界逐元素动画已交付（批次 58，`ui.menu-element-tween.v1`）；LiveNovel 来源菜单的效果映射已交付（批次 59，标题/回想选择音与回想 BGM → 页面点击音效/循环页面音乐），悬停音效/动画光标与来源动画映射仍待完成 |
| P5 有限组合与故事交互 | Sequence/ParallelAll（批次 50）、类型化结果与语义游标（批次 51）、第二来源（LiveNovel 選択メニュー）复用同一核心（批次 52）均已交付；关卡三条全部满足 |
| P6 兼容认证与困难案例 | 三项已交付：ImportReport 映射级别与证据类（批次 53，报告格式 2 + 显式近似接受门禁）、完整路线认证（批次 54，Player 级实包 Auto/回想锁与入口/按住快进/隐藏/菜单切换/演出中存读档全路线，含 256 MiB 内存账本修正）、能力发行清单（批次 55，43 能力逐项执行/恢复/后端证据 + verify_capabilities.py 门禁） |

Windows 宿主已同步修改，但 Linux 上的公共 Rust 测试不覆盖 cfg(windows) 原生运行路径；不得据此声明 Windows 实机验收通过。实际测试日志保留在本地 `reports/nir-next/`。

## 本批验证

- 修改前 Rust 基线 179 项通过；任务原因/事件增益新增 4 项 Core 回归，另补导入器媒体配方复用测试。
- 最终 Core/Player/Format 共 122 项测试通过；Compiler 定向测试 28 项通过（1 项需外部样本的用例忽略），音频契约 4 项通过。
- Host 61 项通过；Clippy（Core/Player/Compiler/Engine 全目标）、依赖架构和差异检查通过。
- 独立 SDK/CLI 已构建且独立交付验证通过（包含 PATH 为空的导入/构建验证），中性项目以非单位事件 gain 正式编译发行。真实 Chromium/WebGL2 验证初始增益、修改玩家音量、保存/恢复后增益，无浏览器错误。
- 未验证 Windows 真机或硬件 WebGPU；本次不将这些后端标为已验收。

## 第二批：可恢复的淡出停止

- 增加 `AudioStop { target, duration_us }`，复用既有任务/Await；绑定具体实例，补齐零时长、自然结束先到、Cancel/Finish、scope 退出、冲突与恢复规则。
- Web 使用独立包络 GainNode，原生使用逐采样帧包络；恢复前先设置初始包络，避免从 1 瞬时起播。原生采样算法可在 Linux 单测，但不替代 Windows 设备验收。
- 修复 Web 手势解锁绕过菜单暂停的问题；设置/保存等动作不能恢复已暂停音频。
- 导入器保留 BGM 停止时长和翻页语音 50 ms 淡出，停止句柄按通道复用，避免长篇任务名累积。
- `scripts/serve_nir_next.py` 和 `playwright.nir-next.config.js` 建立可重复的中性浏览器验证入口，不依赖私有工程。
- 本批完整 Rust 检查：194 项通过；Host 61 项通过；新浏览器回归 2 项通过，包含音量相乘、暂停、保存/恢复剩余包络与会话清理。Clippy、架构与差异检查通过；SDK 已重建，独立交付验证通过。日志在 `reports/nir-next/stop-*.log`。

后续仍按完整计划推进 P1.2/P1.3 共享目标与分域暂停、P2 阅读/消息框/转场、P3–P6 页面与多来源认证；不将本批通过视为整个计划完成。详细音频契约见 [音频语义](AUDIO-SEMANTICS.md)。

## 第三批：共享属性求值基础

- 在既有 nir-format 内抽取 `interpolate` 和 `ScalarTween`，复用 Linear/Smooth、Finish/Cancel 政策，不新增调度器或 crate。
- Core 的 Clip 采样/结算与音频停止/恢复、Windows 采样包络共用插值规则。所有者仍持有自己的时钟与状态。
- 中间值以 f64 求值，避免两个有限 f32 端点相减溢出；保留精确端点和零时长行为。
- Format/Core/Player/Windows 公共代码测试通过，日志为 `reports/nir-next/shared-tween-tests.log`。这不覆盖 Windows 原生宿主，也不表示类型化目标或 UI 时钟已经交付；SDK 尚未针对本批重新构建。

## 第四批：类型化场景与消息根目标

- 增加 `tween.target.v1`：SceneNode 的 x/y/scale/opacity、DialogueRoot 的整体/背景/文字 opacity。旧 Clip 与新 Tween 归一化后共用求值和占用规则；没有新增 TaskState 或调度器。
- 消息根按任务 Scope 存活，不误用场景代次：session 效果跨场景继续，scene 效果随场景取消。整体 alpha 是组件绘制 alpha 的共同乘数，不声明离屏分组混合。
- 快照保存外观基础值和活动轨道；检查属性范围、活动 writer、过期进度、缺失节点及未来场景代次。菜单沿用当前 Story 暂停，仍不等于 Story/UI 分域时钟已交付。
- 源/惰性模块均检查能力声明，编译器按实际使用声明此能力。Schema 已重新生成，同时补齐先前消息框/图片菜单变更遗漏的示例 Schema。
- SDK 模板附带 `AUDIO-SEMANTICS.md` 与 `TWEEN-SEMANTICS.md`；公共浏览器 fixture 同时运行动画与音频停止，验证消息画面变化、菜单暂停及恢复。
- 新契约测试覆盖三通道、零时长、Cancel/Finish、替换捕获、作用域、恢复与非法输入；两个旧路线的 Clip/Tween trace 一致；Player 验证颜色相乘且不影响场景。
- 最终 Rust 回归 205 项通过（含 Windows 公共代码，不含原生设备路径），浏览器/WebGL2 3 项通过；实际像素验证消息根渐变而场景不变，并检查菜单暂停和保存/加载后的外观保持。日志：`tween-all.log`、`tween-browser-final.log`。
- 最新 SDK/CLI 重建及独立交付验证通过，包含无工具链环境的导入、确定构建与发行校验；日志：`tween-sdk-verified.log`、`tween-sdk-verify.log`。格式、Clippy 与依赖架构检查通过。硬件 WebGPU 与 Windows 实机仍未验收。

MESON/MESOFF 尚未改为新动画：来源等待/中断规则待认证，继续明确保留旧映射局限；本批不能视为 P2.3 整体完成。后续重点是 P1.3 分域时钟/音频暂停，以及 P2.2 阅读与语音实例边界；P3–P6 保持完整待交付范围。

## 第五批：分域暂停与音频实例身份

- 新增 Story/Foreground UI 时间域；前台暂停与 Story 暂停使用独立令牌。菜单暂停 Story，后台/设备丢失/队列溢出暂停两域，交错解除不互相释放。
- Player 增加前台逻辑时钟与有界时钟请求 token。活动 UI owner 可以请求菜单下的 Tick；未请求时不维持空帧循环。UI 的 Tick 不会意外推进暂停的 Core。前台状态保持瞬态，不写进剧情快照。
- 音频 Start/Envelope/Stop 绑定 `(domain, session, task)`，Pause/Reset 明确域；完成/失败事件同样携带域。前台回调不能碰撞同编号的剧情任务。新宿主与运行时须配套构建。
- Web 使用两个实际 AudioContext，分别管理暂停与手势解锁；Windows 按域筛选 Sink 并按完整身份索引。未知域及非法实例标识拒绝处理。
- Rust 本批 208 项通过，Host 64 项通过；Clippy、格式与依赖架构检查通过。浏览器新增真实双设备时钟回归，与音频/消息动画共 4 项通过；原始证据在 `reports/nir-next/domains-*.log`。
- 页面音乐配置、UI 属性动画 owner 与关闭事务仍未实现；浏览器新增用例明确使用原创静音缓冲验证宿主时钟，不冒充声明式页面交付。Windows 真机和硬件 WebGPU 仍待验收。
- 最终配套 SDK/CLI 重建、独立交付验证和 4 项浏览器验证通过：`domains-sdk-verified.log`、`domains-sdk-verify-final.log`、`domains-browser-final.log`。

详见 [时间域语义](TIME-DOMAINS.md)。下一步先检查 `Core::needs_clock` 当前不为单独 Audio 请求 Tick 时，停留阅读期间的音频保存 offset 是否准确；该路径不在本批以动画/淡出保持时钟的浏览器场景覆盖范围内。随后继续 P2.2 的阅读/语音实例边界，并保持 P3–P6 的完整实施范围。

## 第六批：阅读语音关联与音频恢复位置

- 先通过失败测试确认 `needs_clock` 排除独立 Audio 会使恢复 offset 停滞，再让活动音频保持 Story Tick。完全揭示的对白停留时 offset 继续累积，菜单暂停仍冻结；低频音频时钟优化尚未实施。
- 新增 `text.voice-binding.v1` 和 DialogueVoice 操作，绑定活动 Dialogue 与具体非循环 Voice 实例；支持明确无语音、Gate 后重新绑定、并行等待或语音结束后计时。未绑定的旧作品维持总线等待策略。
- 快照保留绑定/版本并固定原实例，同名句柄复用不会转向新声音；回收保留被引用任务元数据。恢复拒绝坏类型、缺失引用、零绑定版本、缺能力和未提交对白中的伪造绑定。
- Player 的 Auto 计时按会话/交互/绑定版本隔离。LiveNovel 页首和页内事件后生成显式绑定，源页完成沿用 50 ms AudioStop；布局视口翻页不被当作源页完成。
- SDK 增加阅读语义文档并同步 Schema；额外补齐此前未导出的 module-code Schema，使惰性执行代码的操作也有完整格式描述。公共浏览器入口增加独立阅读工程，不依赖动画或淡出替音频维持时钟。
- Rust 219 项通过，Clippy、格式与架构检查通过。5 项浏览器检查包含新的真实 offset 保存/恢复和原有音频/消息动画/双域回归；实际设备采样位置与恢复起播位置使用 200 ms 容差，不声明样本级同步。
- 最新 SDK/CLI 重建与独立交付验证通过。证据：`reading-all-final.log`、`reading-clippy-final.log`、`reading-sdk-verified.log`、`reading-sdk-verify-final.log`、`reading-browser-final.log`；Windows 实机与硬件 WebGPU 仍未认证。

语义见 [阅读语义](READING-SEMANTICS.md)。后续继续玩家阅读偏好与输入、消息表现/遮罩、P3–P6 的页面、Replay、有限组合及多来源认证；本批不代表整个计划或全部 P2.2 完成。

## 第七批：阅读偏好与可滚动设置

- 新增玩家 `text_speed` / `auto_wait_scale`，范围 0.25–4，默认 1；旧偏好记录自动补默认值。它们属于引擎阅读偏好，不增加剧情指令或作品能力声明。
- 新 Dialogue 在准备时冻结揭示间隔；改变偏好不重算当前 Dialogue，Gate/视口翻页不重新采样。快照保存冻结值并验证范围，旧快照缺字段沿用作者间隔；声音、延时和动画时间不随字速改变。
- Auto 在实际开始等待时冻结倍率；并行和语音后等待的采样边界不同。中途改设置、进入/退出菜单不重算已有周期；重开 Auto、手动推进、交互或绑定 revision 改变重置周期。
- 设置页增加字速、自动等待控件及生效提示；正文有界滚动，标题/关闭入口固定，绘制和可点击范围共同裁切。设置滚动不关闭 Auto。Web 启动读取和持久化包含新字段；Windows 共用 Preferences 序列化及设置投影，未验收原生设备。
- 完整编译验证发现中性示例字体缺少新 UI 文案字形，已从仓库现有 OFL 字体母版补齐子集；没有绕过覆盖检查。Schema 与配套 SDK 已更新，独立交付验证通过。
- Rust 回归 231 项通过（本批包含 presentation 包），Host 64 项通过；Clippy、格式与依赖架构检查通过。新增 Core/Player 回归覆盖冻结/恢复、非法冻结值、周期内改偏好、新会话继承和三种视口下控件可达性。
- 浏览器/WebGL2 原有 5 项通过；新增设置用例最初因测试选择器依赖 JSON 字段顺序超时，修正后单独重跑通过，验证实际语义控件、横屏滚动和刷新持久化。另检查横屏截图的裁切和固定关闭入口。证据：`preferences-all-final.log`、`preferences-host.log`、`preferences-clippy-final.log`、`preferences-sdk-final.log`、`preferences-sdk-verify-final.log`、`preferences-browser-final.log`（原有 5 项）、`preferences-browser-controls.log`（新增 1 项）、`preferences-landscape.png`，均位于本地忽略的 reports/nir-next。

详见 [阅读语义](READING-SEMANTICS.md)。P2.2 的共享输入路由、背景推进、临时隐藏、Ctrl held 与失焦释放仍待实现；P2.3/P2.4 及 P3–P6 保持完整待交付范围。本批不代表整个计划完成，也未提交或推送。

## 第八批：按住快进与失焦释放

- 新增共享 Player 输入 HoldSkip，瞬时按住状态与 ToggleSkip 分离。只在 Story 未暂停且无选择时接受按下；已读、硬 Gate 和选择限制沿用唯一 Core/Player 路径。松键不关闭独立的锁定快进，菜单/后台/会话切换/恢复解除按住状态。
- Web 接入左右 Ctrl、keyup、blur、后台和编辑控件焦点；IME/编辑控件优先。无活动按键时不发送多余释放或触发音频解锁。Windows 映射相同语义，左右键分别跟踪；失焦/遮挡按或关系暂停。另补 Windows 设置页滚轮目标。Windows 原生代码尚未在实机认证。
- Rust 234 项通过，Host 64 项通过；共享 Player 用例覆盖已读推进、未读拦截、锁定/按住独立、菜单和后台释放。Clippy、格式和架构检查通过。SDK 与 Schema 已更新。
- 浏览器整套运行中 6 项通过，新增实际 Ctrl 用例验证了持久化已读记录、松键/blur 取消，以及编辑控件焦点优先。最终输入用例单独复验记录在 `held-input-final.log`；配套 SDK 独立验证为 `held-sdk-verify-final.log`。其余证据：`held-all.log`、`held-host-final.log`、`held-clippy.log`、`held-sdk-final.log`。
- **新增未解决证据**：音频位置恢复用例整套运行两次失败，差值约 0.98 s / 0.61 s，超过 200 ms 容差；独立运行通过。证据在 `held-browser-final.log`、`held-browser-verified.log`、`held-reading-isolated.log`。不能以独立通过替代整套失败，也不能声称本批全套浏览器通过。Web、Windows 和 Player 均有 250 ms 时间截断路径，怀疑主线程停顿导致逻辑时钟落后；尚需确定性停顿实验定位和修复，下一批优先处理。

P2.2 的完整共享焦点路由、背景推进、临时隐藏及全布局就绪提示仍未完成；P2.3/P2.4、P3–P6 保持完整范围。本批未提交或推送，不代表全计划完成。

## 第九批：停顿后的时间保留与预算续算

- 用浏览器主动停顿主线程 800 ms 稳定复现上批计时问题：再等待 500 ms 后，Story 仅累计约 743 ms；Player 单元测试也确认输入 900 ms 只推进 250 ms。失败证据：`clock-stall-before.log`、`clock-unit-before.log`。
- 去除 Web、Windows 和 Player 对经过时间的 250 ms 截断。宿主 u32 Tick 的单次表示上限仍保留，超出部分留待下一轮；Core 继续按既有工作预算处理，不因停顿取消预算限制。
- Core 因预算让出时，剩余剧情时间改用独立 ContinueStoryTime 事件，避免重复推进 Foreground UI 时钟。续算沿用同轮输入优先及会话身份检查，新会话拒绝旧时间。暂停、准备和输入切点政策未改写。
- 新回归比较小预算/普通预算下的完整音频 elapsed，并验证前台时间只累计一次、切换会话丢弃旧续算。Rust 236 项通过；Host 64 项、Clippy、格式和依赖架构检查通过。
- 浏览器音频恢复用例保留 200 ms 容差，增加确定性的 800 ms 主线程停顿；配套 SDK 下整套 7 项通过。独立 SDK/CLI 交付验证通过。证据：`clock-all.log`、`clock-host.log`、`clock-clippy.log`、`clock-sdk.log`、`clock-sdk-verify.log`、`clock-browser.log`。Windows 原生设备和硬件 WebGPU 仍未认证。

上述结果证明活动阅读期间发生停顿后可保留经过时间，并在恢复调度后正确保存音频位置；不声明样本级设备同步，也不扩大到尚未交付的页面效果生命周期。后续继续 P2.2 背景推进、临时隐藏和焦点路由，以及 P2.3/P2.4、P3–P6；完整计划保持进行中。

## 第十批：共享背景点击与主键路由

- 将指针与无焦点 Enter/Space 的动作判定移到 nir-presentation，共享 Engine 接口供 Web/Windows 调用，删除宿主中重复的动作推测。
- 左键命中最上层语义区域，禁用区域阻挡穿透；空白背景仅在未暂停、未加载、无选择且有对白的 Story 请求 Advance。实际推进仍经过 Engine 的视口翻页和唯一 Core 的 Gate/交互检查。脚本隐藏对白保留已有继续入口。
- 右键在 Story 打开菜单、在系统子页关闭；标题不触发入口。主键只使用实际可用的新游戏/继续入口，不在子菜单自行开始或继续剧情。加载只阻挡推进，不封锁可见取消/恢复控件。Web 编辑控件与 IME 先消费按键。
- Windows 点击增加按下/抬起目标、移动距离和会话/交互一致性检查，Web 同步检查身份并在离开画布时取消手势；拖动与旧界面松键不会激活背景。Windows 原生设备仍未认证，完整 Tab/方向键焦点导航仍待交付。
- 两项原宿主路由测试迁入共享 Rust 并扩展边界，因此 Host 测试数从 64 变为 62。Rust 全回归 238 项通过；最后的路由修正另经 Player/Presentation 回归通过。Clippy、格式与架构检查通过。
- 浏览器新增实际背景点击揭示/推进、菜单拦截、编辑焦点优先和拖动取消。音频停顿测试曾在固定 500 ms 等待后读到尚未完成下一轮调度的状态，改为等待实际完成的 owner turn；恢复位置的 200 ms 容差不变。最终配套 SDK 下整套 8 项通过，独立 SDK/CLI 验证通过。
- 证据：`input-all.log`、`input-player-final.log`、`input-routing-final.log`、`input-host-final.log`、`input-clippy-final.log`、`input-sdk-final.log`、`input-sdk-verify-final.log`、`input-browser-verified.log`，均位于 reports/nir-next。

临时隐藏的显式政策、全布局静态就绪提示与完整焦点导航仍待推进；P2.3/P2.4 及 P3–P6 保持原范围。完整计划未完成，本批未提交或推送。

## 第十一批：临时隐藏、显式政策与静态提示

- 增加玩家独立呈现遮罩，工具栏 Hide / H 进入；首次指针、主键或菜单操作只恢复显示，不推进对白。关闭 Auto/锁定快进/按住快进，不改写脚本显隐、消息根外观或 Core 快照。会话切换、恢复、选择和阻塞故障清除遮罩。
- 默认 ContinueStory 保持 Story 和声音运行。新增显式 PauseStory 配置及 `player.hide-policy.v1`；采用独立暂停 owner，恢复不解除后台/设备等其他暂停。编译器按实际配置推导声明，源与 runtime root 均拒绝缺声明。当前不提供“仅暂停某个音轨”等未经定义的组合，导入器也不自动推断原引擎政策。
- 资源/语言准备包含解除玩家遮罩后的呈现，防止恢复显示时缺少已准备文字。脚本隐藏仍保持独立。原生 H 不处理 Ctrl/Alt/系统修饰组合，Windows 实机仍未认证。
- 自定义 rect 与标准对白布局都增加静态阅读提示，优先使用框外空间，无外部空间时用底角覆盖提示，不改变正文几何；有滚动时连同提示底板移除并使用翻页控件。三个视口和全屏默认留白经单元检查，自定义框截图已人工检查。
- Rust 全回归 244 项通过，最终遮罩恢复/故障及提示边界修正另经 Player 回归通过；Host 62 项、Clippy、格式和架构检查通过。配套 SDK/Schema 更新及独立交付验证通过。
- 新浏览器 fixture 使用正常编译的 PauseStory 和自定义对白框。两项隐藏用例在最终 SDK 下通过，分别验证设备音频时钟继续/冻结和恢复不推进；证据为 `hide-controls-final.log` 与 `hide-custom-ready.png`。
- **音频同步仍未完全解决**：整套浏览器运行 9 项通过，音频恢复用例仍失败，位置差约 1.33 s（`hide-browser.log`）。此前去除 250 ms 截断解决了确定性丢时间，但不足以证明实际设备起播/暂停与逻辑时钟的全面同步。已给恢复断言加入设备起播时刻、恢复 offset 和 Story 时间诊断，下一步优先确定性复现起播时点偏差；不能把本批声明为全套浏览器通过。
- 其余证据：`hide-all.log`、`hide-player-final.log`、`hide-host.log`、`hide-clippy-final.log`、`hide-sdk-final.log`、`hide-sdk-verify-final.log`。上述日志位于本地忽略的 reports/nir-next。

原生完整焦点导航、来源隐藏政策认证，以及 P2.3/P2.4、P3–P6 继续保持原范围。完整计划未完成，本批未提交或推送。


## 第十二批：设备播放位置与剧情时间分离

- 延迟首次起播 800 ms 的实验未复现误差；同一回调内停顿 800 ms 后立即打开菜单，稳定复现约 789 ms 恢复偏差。输入优先会暂停 Story，设备在未处理帧时间内仍播放，因此单靠任务 elapsed 不足以恢复真实播放位置。修复前证据：`audio-anchor-before.log`、`audio-pause-before.log`。
- 增加带域/会话/任务身份的设备位置观测，Web 读取 AudioContext 播放时间，Windows 读取 Sink 位置并加恢复基值；在动作提交和 Tick 前采样。Engine 直接更新元数据，不运行剧情；Core 不改变逻辑时钟、elapsed 或里程碑。拒绝重复/超限批次，忽略旧会话、前台域、非音频和已终止任务。
- 尚未发行的快照 v2 增加可选 audio_position_us，恢复音频优先使用该值；旧快照缺字段仍使用 elapsed。新增 Core/Player 测试验证观测无剧情副作用、非法快照、身份隔离和新旧快照恢复。此项是宿主设备协议，不增加作者指令或 requires 能力。
- 修复只针对实际播放位置；停止包络的剩余进度仍按现有任务语义恢复，不宣称物理包络、Story 与设备输出已达到样本级同步。Windows 路径已接入，仍未进行 Windows 实机认证。
- Rust 分组回归共 246 项通过，Host 62 项通过；Clippy、格式、依赖架构检查与独立 SDK 验证通过。证据：`audio-position-rust.log`、`audio-position-other-rust.log`、`audio-position-host.log`、`audio-position-clippy.log`、`audio-position-sdk.log`、`audio-position-sdk-verify.log`。
- 首次整套浏览器运行的音频停顿/恢复用例通过，但隐藏继续播放用例在固定 350 ms 内只得到 171 ms 设备进度；已将该用例改为有界等待实际前进超过 200 ms，不将设备吞吐误当隐藏暂停语义。暂停冻结检查和恢复位置的 200 ms 容差保持不变。初次结果保留于 `audio-position-browser.log`。最终配套 SDK 下整套 10 项通过，包含两种隐藏政策与三处主动 800 ms 停顿；证据为 `audio-position-browser-final.log`。

完整计划继续进行；原生焦点导航、前台 UI owner 的完整生命周期、消息/遮罩、页面组合、Replay、有限组合和第二来源认证仍未全部完成。本批未提交或推送。

## 第十三批：前台时间的宿主分发

- 查实 Web 帧循环原先仅在 Story 未暂停时发送 Tick，导致 Player 虽支持前台时间域，菜单下却收不到时间。增加 TickDomains 与配套 Engine/WASM 接口，旧 Tick 保留兼容包装。
- Web 分别累计剧情与前台经过时间。菜单进入/停留/退出只丢弃剧情边界时间，前台继续；后台、会话替换丢弃两域旧时间。等待输入和 u32 分批分别保留两域余量。Player 继续以暂停令牌复查，预算续算只补剧情时间。
- 新宿主测试覆盖菜单三种边界、输入延迟、分批余量、后台和会话替换；Player 测试确认返回菜单后的旧时间不计入剧情。真实浏览器在菜单内请求正常 owner turn，检查前台逻辑时间增长、Story 不动，并沿用真实设备分域暂停检查。
- Player/Engine 回归 96 项、Host 65 项、浏览器整套 10 项通过；Clippy、格式、依赖架构、配套 SDK 构建和独立交付验证通过。证据：`domain-dispatch-rust-final.log`、`domain-dispatch-host.log`、`domain-dispatch-browser.log`、`domain-dispatch-clippy.log`、`domain-dispatch-architecture.log`、`domain-dispatch-sdk.log`、`domain-dispatch-sdk-verify.log`。

此批补齐 Web 分域时间通路，不声明自定义页面效果所有者、Close 动画或菜单音乐已交付。Windows 原生设备与硬件 WebGPU 仍待认证。完整 P0–P6 计划保持进行中，未提交或推送。

## 第十四批：正文单层阴影

- 主题 dialogue 增加可选 shadow，偏移每轴有限 -16..16，RGBA 有限 0..1；缺省无阴影。新增 `text.shadow.v1`，编译器按实际主题配置推导，源 Program/runtime root 均拒绝缺声明。Schema 和配套 SDK 已同步。
- 阴影只增加绘制项，不增加阅读区域、分页或辅助语义。沿用相同字体计划、文字分段、字素揭示、滚动和正文裁切；自定义舞台缩放作用于偏移，正文有效 alpha 作用于阴影。临时隐藏与恢复沿用整体显示政策。
- 强调色不能直接复制到阴影，因此塑形缓存区分单色绘制，同时保留分段；单元测试比较实际字形位置与 ID，确认阴影和正文一致且无强调颜色。无阴影时不复制文字集合；有阴影时最多增加一份单色塑形与绘制，沿用缓存回收。
- Core/Format/Compiler/Player/Engine 已通过分组测试 234 项；Presentation 最终 11 项通过。早期一项精确浮点裁切断言因 27.9 与 27.900002 差异失败，改为 1e-5 几何容差后复验通过，字形坐标对照仍严格相等。证据分别为 `shadow-rust-final.log`、`shadow-geometry.log`、`shadow-presentation-final.log`。
- Host 65 项、浏览器整套 11 项通过；新增浏览器用例检查实际阴影颜色像素，以及隐藏后消失/恢复后重现且对白实例不推进。截图 `shadow-ready.png` 已人工检查。Clippy、格式、架构与独立 SDK 交付验证通过；证据为 `shadow-host.log`、`shadow-browser.log`、`shadow-clippy.log`、`shadow-architecture.log`、`shadow-sdk-final.log`、`shadow-sdk-verify.log`。

契约见 [阴影语义](TEXT-SHADOW-SEMANTICS.md)。当前作用于正文；来源字体/样式设置仍未自动映射，不据此声明原作样式已经精确迁移。Windows 实机与硬件 WebGPU 未认证。消息来源语义、遮罩、页面、Replay、有限组合与第二来源等完整计划范围继续保留，未提交或推送。

## 第十五批：共享键盘控件焦点

- Presentation 增加瞬态 KeyboardFocus，Tab 顺序/反向、四方向空间导航共用规则，跳过禁用和屏幕外控件。焦点绑定会话、交互、页面、控件 ID 与动作，重用数字 ID 不继承旧动作。
- Engine 使用有效焦点处理主键，并在最终投影中绘制边框；投影失效清除焦点。Windows 接入 Tab/方向键/主键，并在点击、失焦时清理。Web DOM 按钮接入同一路导航，保留编辑/IME 优先及列表边界的浏览器 Tab 出口，不把用户困在播放器内。
- Rust Presentation/Player/Engine 109 项通过；新增用例覆盖空间/顺序移动、禁用与屏幕外排除、旧会话/交互/页面/复用 ID 拒绝。Host 65 项通过，Clippy、格式、依赖架构通过，SDK 与独立交付检查通过。
- 浏览器整套 12 项通过，新增用例实测 Tab、Shift+Tab、方向键、聚焦 Settings 后 Enter 激活及编辑框优先。证据：`focus-rust.log`、`focus-host-final.log`、`focus-clippy.log`、`focus-architecture.log`、`focus-sdk-final.log`、`focus-sdk-verify.log`、`focus-browser.log`。

当前只导航已投影控件；长列表跨视口的焦点跟随、原生图片 hover 反馈及 Windows 实机仍待交付/验证。此批不表示完整 P2.2 或全计划完成。页面、Replay、遮罩、组合及第二来源等范围保持不变，未提交或推送。

## 第十六批：滚动边界焦点与编号唯一性

- 发现设置页裁切删除控件后，按当前数组长度分配新控件编号会复用已保留 ID。改为在现存最大 ID 后分配，补充裁切后追加控件的唯一性回归；动作身份复查继续保留。
- 在可滚动区域末端/首端，Tab/反向 Tab、上下方向键先滚动一个有界步长，再重新投影并选择新露出的可用控件。没有新候选时尽可能保持原动作；仍使用现有窗口投影，不展开全部长列表绘制项。
- Web 延迟到新语义 DOM 创建后设置焦点，待应用请求带会话、交互、页面身份；指针按下取消待应用焦点。播放器与宿主 Tab 边界依然可以离开，不锁住宿主控件。Windows 导航同步以焦点中心请求图片菜单 hover 反馈。
- Presentation/Player/Engine 109 项先通过，追加唯一性回归后 Presentation 最终 14 项通过；Host 65 项、Clippy、格式、架构检查通过。SDK 与独立交付验证通过。
- 浏览器整套 13 项通过。新增窄屏设置用例只用键盘到达屏幕外字速控件、检查语义编号唯一，再反向返回顶部。证据：`focus-scroll-rust.log`、`focus-scroll-presentation.log`、`focus-scroll-host.log`、`focus-scroll-clippy.log`、`focus-scroll-architecture.log`、`focus-scroll-sdk-final.log`、`focus-scroll-sdk-verify.log`、`focus-scroll-browser.log`。

设置页往返已有实际浏览器证据；长选项、超高控件和后续自定义集合仍待专项验收，Windows 实机仍未认证。其他 P0–P6 范围保持完整待交付状态，未提交或推送。

## 第十七批：冻结场景的方向擦除

- StagePresent 增加可选 transition，缺省 dissolve 保持旧字段与行为；`stage.wipe.v1` 提供四方向及有限 0..1 软边。源/runtime 内容均验证参数和能力声明，编译器按实际使用推导，Schema/SDK 同步。
- 沿用原 StagePresent 任务、前后场景、暂停、终态、恢复与双离屏目标预算；纯函数与 GPU 使用相同覆盖率公式。零/满进度精确输出两端，只改变混合权重，不新增资源或第三份渲染目标。
- Rust 全分组原 248 项通过，新增方向/端点/单调软边和恢复测试后 Core/Format 76 项通过；最终恢复专项 8 项通过，包括伪造样式拒绝。证据为 `wipe-rust.log`、`wipe-contract.log`、`wipe-restore-final.log`。Host 65 项、Clippy、格式、架构检查通过。
- 首次浏览器启动暴露 WGSL 的 target 保留字错误，已修正为 target_color。该次在定位后主动中止，记录保留于 `wipe-browser.log`，不能把 Rust 编译当 shader 验证。修正后重建配套 SDK，并通过独立交付验证。
- 最终浏览器整套 14 项通过。新增红/蓝原创场景检查源端/目标端和中间软边像素，与线性合成参考值比较，容差 15/255；同时检查后台暂停冻结、中途保存/加载保留进度和继续到终点。截图 `wipe-middle.png` 已人工检查。最终证据：`wipe-browser-final.log`、`wipe-sdk-shader-fixed.log`、`wipe-sdk-verify-final.log`、`wipe-clippy.log`、`wipe-architecture.log`、`wipe-host.log`。

契约见 [场景转场语义](STAGE-TRANSITION-SEMANTICS.md)。本批只交付方向擦除，不替代完整 P2.4：纹理阈值遮罩、消息/UI 根、来源参数换算仍待实现；硬件 WebGPU 和 Windows 实机仍待认证。页面、Replay、组合、第二来源等范围保持完整，未提交或推送。

## 第十八批：Alpha 纹理阈值遮罩与资源闭包

- 新增 `stage.mask.v1`，transition.mask 显式引用 Image、Alpha 通道、反向极性与有限软边；最近邻/clamp 采样。Alpha 不受图片上传的 RGB 预乘/颜色转换影响，来源灰度图须由转换器转为 Alpha 数据，当前没有自动来源参数映射。
- 遮罩贯通发行根索引、模块消费者、源/runtime cue 媒体配方、激活准备、运行中留存和冷读档目录/媒体准备；runtime 拒绝配方漏列遮罩。沿用图片预算和现有两个离屏合成目标，额外图片不是第三份场景合成目标。
- 转场定义冻结资源身份、极性、软边；恢复重新校验。新增 Core 用例验证能力/图片类型、准备闭包和中途恢复；Player 用例验证结束后从活动集合释放、恢复后重新留存。Rust 原分组 253 项通过，新增 Core/Format 合同 78 项、Player 留存专项 1 项通过。
- Host 65 项、Clippy、格式、依赖架构与 SDK 独立交付验证通过；Schema 与 SDK 已同步。证据：`mask-rust.log`、`mask-contract.log`、`mask-retention.log`、`mask-host.log`、`mask-clippy.log`、`mask-architecture.log`、`mask-sdk-final.log`、`mask-sdk-verify.log`。
- 浏览器整套 16 项通过，两项新用例使用原创 2×2 Alpha 数据，RGB 与阈值无关；检查正反极性、线性合成参考值，待转场结束/资源退出活动集合后加载中途存档，四个采样点恢复前后完全一致。首次像素通过后发现取样时刻可能位于软边之外，收紧到中段并增加红/蓝必须同时存在的断言；最终两项专项再次通过。证据：`mask-browser.log`、`mask-soft-edge-final.log`。`mask-false.png` 的阈值象限与中间混合结果已人工检查。

契约见 [场景转场语义](STAGE-TRANSITION-SEMANTICS.md)。本批是冻结场景根的纹理遮罩；消息/UI 根、来源通道转换与参数换算、动态/live 输入仍未交付，硬件 WebGPU/Windows 实机未认证。完整页面、Replay、有限组合与第二来源等计划保持进行中，未提交或推送。

## 第十九批：有限菜单静态组合与交错绘制

- 新增 `ui.menu-elements.v1`：ImageMenu 的可选 elements 支持 Group、Image、Text、Button、HitRegion，默认空数组保留旧格式。源/runtime 检查能力、资源与无参数/无返回入口；编译器按实际使用推导，图片所有状态进入发行/准备闭包，Schema/SDK 同步。
- 元素共享场景层级的变换、继承 opacity 和裁切；语义矩形取相同裁切。每菜单旧按钮与元素总量 256、文字 64、层级 8，并限制文本、坐标、缩放和颜色。禁用控件消费命中，透明度不隐式关闭交互。
- 原渲染器把所有文字统一画在图片后，不能表达原菜单图文交错；新增有界菜单绘制顺序及独立文字批次，共用 atlas，按声明的子树顺序交错绘制。旧背景/按钮先画，播放器覆盖控件后画，不增加离屏目标。
- 修正 hover 根据动作寻找第一个按钮的问题：投影保存语义 ID 对作者控件 ID 的映射，直接使用命中身份。因此同动作的两个按钮仍能分别高亮；旧图片菜单也走同一身份路径。
- Rust 分组 256 项通过（1 项既有测试忽略）；最终元素合同 3 项再通过。覆盖变换、裁切、透明度、顺序、Profile guard、非法层级/重复 ID/能力/资源/入口及文字预算。Host 65 项、Clippy、格式、架构、SDK 独立交付检查通过。证据：`menu-rust.log`、`menu-contract-final.log`、`menu-host.log`、`menu-clippy.log`、`menu-architecture.log`、`menu-sdk-final.log`、`menu-sdk-verify.log`。
- 浏览器整套首次 16 项通过、1 项旧焦点测试失败。失败测试连续 Tab 未等下一帧 DOM 焦点更新，漏过目标；增加每步焦点改变等待后，两项焦点专项通过。新增菜单测试实测文字前/后的半透明蓝图像素、两个同动作按钮 hover、锁定命中不穿透及设置动作。证据：`menu-browser.log`、`menu-focus-final.log`。未把首次运行记为全套通过。

契约见 [有限菜单元素语义](MENU-ELEMENTS-SEMANTICS.md)。此批完成 P3.1 静态组合的基础部分：锚点/额外文字测量、P3.2 局部状态与纯绑定、P3.3 值控件/集合/服务身份均未交付。来源原图对照、硬件 WebGPU 和 Windows 实机尚未认证；页面效果、Replay、有限组合、第二来源及其他 P0–P6 范围继续待实施，未提交或推送。

## 第二十批：菜单局部值、纯条件与实例化输入

- 新增 `ui.menu-state.v1`：每菜单最多 32 个 Bool、带上下界 I32 或有限枚举局部值；元素可用有限 Local/Profile 条件做显隐/启用，Text 可绑定完整局部值，SetLocal 只原子赋值一个已声明字段。源/runtime 均静态检查类型、域、引用和能力，编译器按实际使用推导。
- Player 持有瞬态菜单实例、revision 和 locals。新元素及带状态旧按钮使用实例/版本/控件凭据；执行时复查加载、Profile、祖先条件和赋值边界，拒绝迟到/隐藏/禁用输入。普通菜单覆盖页返回保留值但换实例；图片菜单重新打开或 session 更新重置值。具名菜单/入口不允许裸动作绕过新控件条件。
- 投影控件 ID 按完整声明位置稳定分配，焦点跨 revision 保持同一 instance/control，实际激活使用最新动作。Web 的 DOM 重建也使用这份焦点身份；实例变化、隐藏或禁用仍清除焦点。补齐枚举所有可显示值与控件标签的作者字体收集。
- Rust 核心/播放器/呈现等 196 项、编译器 65 项通过（另有 1 项需要私有来源的既有测试忽略）；新增局部合同 4 项及字体字集 1 项。Host 66 项、Clippy、格式/架构、配套 Schema/SDK 与独立发行检查通过。Linux 上 player-windows 包类型检查通过，不代表 Windows 专属宿主或真机认证。证据：`menu-state-rust-final3.log`、`menu-state-compiler.log`、`menu-state-host.log`、`menu-state-clippy.log`、`menu-state-architecture.log`、`menu-state-native-check.log`、`menu-state-sdk-final.log`、`menu-state-sdk-verify.log`。
- 浏览器 18 项中 17 项通过：新用例实测三页签、父 Group 条件显隐、选槽后禁用、旧 revision 拒绝和键盘焦点保留。旧音频恢复测试失败，设备淡出值与读档后值差约 0.195，超出 0.1 断言；保留原断言，未通过重跑掩盖。证据：`menu-state-browser.log`。

契约见 [菜单局部状态](MENU-STATE-SEMANTICS.md)。此批是 P3.2 基础，不包含 params、Preferences/SaveSlots/History 只读模型、故事导出、范围控件、集合窗口或完整服务事务；示例选槽并未实际保存。完整计划继续实施，未提交或推送。

### 下一步优先修复：设备包络与恢复进度

设备播放位置已经作为观测数据保存，但 `Core::audio_envelope` 仍只用 AudioStop 的逻辑 elapsed 推算恢复包络；Web Audio 的 GainNode 在主线程停顿时继续前进，输入优先进入菜单时 Story elapsed 不追补，造成恢复音量跳变。这一限制此前已记录在音频语义文档，现有浏览器失败给出实际证据。必须明确设备包络观测/完成与任务语义时钟的关系，同时覆盖取消、终点、暂停、恢复与两宿主；不能简单放宽像素/音量容差，也不能通过任意推进剧情来消除偏差。

## 第二十一批：设备包络检查点与无跳变恢复

- 修复第二十批暴露的读档音量跳变：AudioEnvelope 命令携带具体 AudioStop owner 和累计设备进度；AudioPosition 可回传该包络检查点。Core 先验证整批、只更新仍拥有该 Audio 的运行中停止任务，拒绝越界并防止进度倒退；旧 owner/会话不能覆盖新的包络。
- 停止任务在快照 v2 中可选保存 `audio_device_elapsed_us`。恢复当前值与剩余线性段、Cancel 的 CommitCurrent 优先使用设备进度；旧快照无检查点时沿用逻辑 elapsed。常值替换清除设备 owner。验证拒绝非停止任务上的检查点及超过声明时长的值。
- Web 按 AudioContext 时钟采样，暂停不走时；Windows 包的共享采样源按已经产生的音频帧采样，两声道同一帧保持同值，恢复从累计起点继续。没有改变 Story tick、任务逻辑 elapsed、Await 或作用域完成规则；设备先归零时仍等待原任务收尾，不因观测执行剧情。
- Rust 核心等整组 263 项通过（另有 1 项私有来源测试忽略），原生包 6 项通过；新增合同覆盖快照只变观测字段、旧快照回退、非法检查点、取消保值与迟到 owner 拒绝，采样测试覆盖恢复累计起点与常值替换。Host 67 项、Clippy、格式/架构与独立 SDK 发行验证通过。证据：`envelope-rust.log`、`envelope-contract-final2.log`、`envelope-host.log`、`envelope-clippy.log`、`envelope-format.log`、`envelope-architecture.log`、`envelope-sdk-final.log`、`envelope-sdk-verify.log`。
- 浏览器音频用例加入明确 800 ms 主线程阻塞后立即进入菜单，读档音量误差从原小于 0.1 收紧到小于 0.01，保留暂停、玩家 bus gain、剩余淡出、恢复对白属性与会话退出检查。该用例通过，浏览器整套 18 项全部通过，证据为 `envelope-browser.log`。

契约更新于 [音频语义](AUDIO-SEMANTICS.md)。这是设备呈现检查点修复，不表示音频设备与剧情时钟已经全局同步，也不替代 Windows 音频设备和硬件 WebGPU 实测。P3 系统服务、页面效果、Replay、有限组合、第二来源及其余完整计划仍待实施，未提交或推送。

## 第二十二批：自定义剧情菜单与偏好服务

- 新增 `ui.menu-services.v1`：主题可用 `menu_overlay` 声明具名剧情菜单，也可仅替换系统菜单、保留默认标题。有限 `text_preference` 只读绑定及调整偏好、减少动态效果、关闭动作复用已有设置服务和持久化；字号、音量仍走原排版与 bus gain 路径。编译器按实际使用推导能力，源/runtime 同步验证。
- Menu 暂停并返回原 Story；覆盖页导航不改标题菜单位置。局部值、实例和 revision 分离，旧页面或旧偏好版本的输入被拒绝；同一定义同时用于标题和覆盖页时，局部值仍隔离，返回保留标题值。覆盖页 Entry 闭包被拒绝，避免冒充尚未隔离的 Replay。
- 覆盖页专用媒体不在标题预加载；当前页按需准备并留存，导航和关闭释放。保留正在进行的剧情资源事务，离页只取消菜单自己的请求、诊断与暂停标记。故障时提供内置恢复出口；失败注入发现并修复取消后残留 prepare 暂停的问题。
- Rust 初始整组 266 项通过（另有 1 项私有来源测试忽略）；资源恢复补充后核心等子集 202 项通过，最终菜单合同 9 项通过。Host 67 项、最终 Clippy、架构检查、Schema/SDK 构建和独立发行检查通过。证据：`menu-services-rust-final.log`、`menu-services-final-checks.log`、`menu-services-owner.log`、`menu-services-host.log`、`menu-services-clippy-owner.log`、`menu-services-architecture.log`、`menu-services-sdk-owner.log`、`menu-services-sdk-verify-owner.log`。
- 首轮浏览器 18 项通过、新菜单用例失败。定位到测试读取省略的默认 text_speed 后得到 NaN，实际操作已正确写入 1.25；修正测试按偏好合同补默认值，并使用可显示实际结果的轮询断言，没有修改产品以绕过失败。最终浏览器整套 19 项全部通过，覆盖键盘焦点保持、鼠标设置调整、Story 暂停、旧实例/revision 拒绝、关闭返回与重载持久化；证据：`menu-services-browser-final.log`。

契约见 [菜单服务语义](MENU-SERVICES-SEMANTICS.md)。这是 P3.3 的首个服务增量；Range/Toggle 值控件、集合窗口、SaveSlots/History、确认与存读档事务、params/故事导出仍未完成。页面效果、Replay、有限组合、第二来源以及 P0–P6 其余范围继续实施；未提交或推送。

## 第二十三批：槽位读取身份与返回取消

- 为槽位 Load 分配 job，并绑定 slot、session、页面 instance。Web/原生宿主回传带 job 的成功/失败事件，Player 复查并消费当前请求；连点取代、返回重开、旧失败、错槽及重复回执不能改写当前会话。槽位仍为 0–2，保存文件格式不变。
- 槽位读档进入原 Restore 候选通道后，返回或另一导航/替换操作可取消候选、非 locale 资源准备及准备暂停；迟到 PresentationReady 不再提交该候选。保存事务和独立 locale 事务未改为页面附属任务。
- 新增 3 项 Player 合同测试，播放器/引擎整组 112 项通过，Host 67 项通过。Clippy、架构检查、Linux 上原生包类型检查通过；后者不代表 Windows 专属代码已在实际平台执行。SDK 重建和独立发行验证通过。证据：`slot-load-rust-final.log`、`slot-load-host.log`、`slot-load-clippy.log`、`slot-load-architecture.log`、`slot-load-native-final.log`、`slot-load-sdk.log`、`slot-load-sdk-verify.log`。
- 浏览器相关 5 项全部通过：新用例实际延迟 IndexedDB 完成回执，返回剧情后释放回执不切换会话，随后新读档正常提交；既有音频恢复、阅读恢复及自定义菜单服务回归通过。证据：`slot-load-browser.log`。

契约见 [存档服务语义](STORAGE-SERVICE-SEMANTICS.md)。这是 P3.3 服务事务前置修复，不是自定义槽位界面交付。文件导入请求身份、故障确认/返回投影、SaveSlots 模型 revision、覆盖确认令牌、集合窗口和其他完整计划范围仍待实施。未提交或推送。

## 第二十四批：读档故障归属与原剧情恢复

- 取消槽位读档只清除该事务的校验/资源/准入诊断，保留无关宿主故障。内容块失败补 request 归属；候选媒体失败不再投递为原 Core 的 PreparationFailed，只有 Activation 自己的失败通知剧情。
- 发现并修复返回后原事件停在准备态的问题：原 Core 待提交事件在候选取消后重新准备，并能正常显示对白。损坏存档尚未创建候选时则保留原有加载请求；不会将其作为候选资源取消。
- Player 整组 115 项通过，再新增“无效存档保留原准备请求”定向测试通过。新增合同涵盖摘要错误、媒体失败、内容失败、无关诊断保留、原 Core 快照不变及恢复到可播放对白。证据：`slot-fault-owner-final.log`、`slot-fault-existing.log`。最终 Clippy、SDK 构建及独立发行检查通过：`slot-fault-clippy-final.log`、`slot-fault-sdk-final.log`、`slot-fault-sdk-verify.log`。
- 相关浏览器 5 项通过（音频恢复、阅读恢复、自定义菜单与延迟槽位读取），证据：`slot-fault-browser.log`。另有真实损坏存档返回用例通过：修改 IndexedDB 中的摘要触发 E_SAVE_DIGEST，返回后会话/交互不变、错误清除且不暂停；证据：`slot-fault-browser-corrupt.log`。

[存档服务语义](STORAGE-SERVICE-SEMANTICS.md) 已同步。本批修复既有服务的实际故障边界，尚未交付自定义槽模板、覆盖确认和完整 P3.3；完整计划继续推进，未提交或推送。

## 第二十五批：自定义槽位与一次性覆盖确认

- 新增 `ui.menu-storage.v1`：固定槽或 0–2 有界局部 Int 选择器、Text 槽位标签绑定、save_slot/load_slot 服务。复用原三槽 SaveJob/Load 候选与 CAS，不扩大存储；源/runtime 检查能力、引用、范围及互斥绑定，编译器按实际使用推导。
- 进入相关菜单按需刷新槽位。槽位元数据/版本、保存忙碌状态、读取请求状态更新菜单 revision；投影与执行共同拒绝空槽加载、标题保存、忙碌槽及过期控件。列表仅保留三行，迟到列表不倒退已知提交版本。
- 空槽保存直接提交，已有槽先进入内置确认页并移除底层命中/焦点节点。一次性 token 绑定 session、页面 instance、发起控件及具体槽位版本；确认复查权限，Back/Cancel、Load、版本变化、控件禁用使旧 token 失效。正常提交后仍是页面独立的保存事务，跨标签页冲突由既有 CAS 拒绝。版本耗尽明确报错。
- Rust 编译/核心/Player/呈现整组 279 项通过（另有 1 项既有私有来源测试忽略），最终菜单合同 13 项通过；Host 67 项、Clippy、架构、Linux 原生包类型检查、Schema/SDK 与独立发行检查通过。初轮编译因新增中文确认文案缺字失败，已从仓库授权字体补齐示例字体并重新验证；初始槽位模型误增 revision 导致旧菜单首个输入失效，也已修复并通过旧合同回归。证据：`menu-storage-rust-verified.log`、`menu-storage-refresh.log`、`menu-storage-host.log`、`menu-storage-clippy.log`、`menu-storage-clippy-refresh.log`、`menu-storage-architecture.log`、`menu-storage-native-check.log`、`menu-storage-sdk-refresh.log`、`menu-storage-sdk-verify.log`。
- 浏览器整套 22 项通过；新增用例操作局部选槽，实际写入 revision 1，取消覆盖并拒绝旧 token，再键盘确认写入 revision 2，最终从作者菜单读档。覆盖窄屏、放大字号后的确认操作。证据：`menu-storage-browser.log`；最终按需列表刷新改动的菜单定向回归 3 项及独立发行复核均通过，证据：`menu-storage-browser-refresh.log`、`menu-storage-sdk-verify-refresh.log`。

[存档服务语义](STORAGE-SERVICE-SEMANTICS.md) 与能力文档已同步。这是可执行的自定义三槽界面，不等于 P3.3 完成：通用集合窗口/模板、History 只读模型、Range/Toggle、作者确认布局、文件导入请求身份等仍待实施。页面效果、Replay、多来源等完整计划范围继续保留；未提交或推送。

## 第二十六批：有界历史窗口与冻结记录呈现

- 新增 `ui.menu-history.v1`：HistoryWindow 固定文本行模板与实例化 history_page 服务。每窗口 1–16 行，一页作者 Text 加声明历史行数最多 64；偏移是 0–999 范围内局部 Int，引用、范围、样式和能力均在 source/runtime 验证，编译器按实际使用推导。
- Player 只复制可见行，隐藏窗口和普通 Story 不构建历史行列表；内置历史页也只复制原有 3 条，保留原分页边界和长文本滚动。行保留历史冻结 locale/font plan/全文，不请求旧章节正文；Group 变换、透明度、逐行与祖先裁切沿用菜单共享几何。key 在暂停 Story 的页面/session 作用域内按历史位置稳定，跨会话不复用。
- 新增 1000 条历史合同：模型仅生成 16 行，前后翻页稳定 key，旧 revision 被拒绝，Core 快照与 retained assets 不变；另覆盖非法大窗口、总行数、引用及能力缺失。原长文本历史测试改用“新窗口由 Player 提供”的模型合同，仍验证行内滚动及切换空窗口后清除旧滚动；实际内置末页边界由新 Player 合同覆盖。
- 相关 Rust 套件分段共 281 项通过（1 项既有私有来源测试忽略）：编译/Core/格式 145 项、最终 Player/呈现/Engine 136 项。菜单合同 15 项通过。Clippy、Host 67 项、架构、Linux 原生包类型检查、Schema/SDK 和独立发行检查通过。证据：`menu-history-rust.log`（保留旧模型测试首次失败）、`menu-history-rust-final.log`、`menu-history-contract.log`、`menu-history-clippy.log`、`menu-history-host.log`、`menu-history-architecture.log`、`menu-history-native-check.log`、`menu-history-sdk-final.log`、`menu-history-sdk-verify.log`。
- 浏览器整套 23 项全部通过。新增历史用例实测两种冻结文本前后翻页的像素变化及返回原页像素一致、旧动作拒绝、边界禁用、Story tick/交互与资源常驻不变。证据：`menu-history-browser.log`。

契约见 [历史窗口语义](MENU-HISTORY-SEMANTICS.md)。本能力是有限的固定文本行模板，不提供任意子控件集合、可变行高/展开详情或语音重播；Range/Toggle、params、作者确认布局与其余 P0–P6 目标仍待实施。未提交或推送。

## 批次 27：值控件与转换闭环恢复（基础交付）

- Range/Toggle 接入有限绑定、Player 校验、共享投影和两宿主输入。拖动松开提交，取消不写入，旧页面版本拒绝；键盘数值调整与 Web 辅助语义已接通。
- 新增输入边界、重复/旧动作、偏好写入及 Host 身份检查。Rust 相关套件 207 项通过，额外转换元素 TOML 往返测试通过；Host 68 项、Clippy、架构检查及 Linux 原生包类型检查通过。
- LiveNovel 标题/回想的源按钮改为自动输出 MenuElement，保留布局、原素材变体与解锁条件。系统菜单仍是明确的转换缺口。
- 执行顺序调整为能力收尾后优先接通来源转换，禁止以中性手写样例代替自动迁移验收。原包脚本检查记录放本地忽略目录。
- 源音量比例、自动等待绝对时间、动态字体列表及系统截图背景应分别映射；不能把不同量纲绑定到现有偏好后声称保真。

契约：[值控件语义](MENU-VALUES-SEMANTICS.md)。未提交或推送；完整 P0–P6 计划仍未完成。


批次 27 验收补充：

- 浏览器原有 23 项通过；新增值控件首次发现 slider 辅助名称遗漏，已补 aria-label。修复后的焦点两项通过；值控件用例修正样例默认音量假设及浮点比较后通过，覆盖键盘、松开提交、取消手势、旧动作拒绝与开关。证据：本地 `menu-values-browser.log`、`menu-values-browser-final.log`、`menu-values-browser-verified.log`。
- 实际来源重新转换发现可选字段的 null 无法写入 TOML，已统一省略菜单中缺省的 Option 字段，并增加序列化往返测试。再次转换、完整项目加载/编译通过；输出标题/回想均为自动生成元素。转换报告仍是 converted_with_adaptations，不能据此声明系统菜单或原版体验已兼容。
- 最终 Schema/SDK 与独立发行验证通过（`menu-values-sdk-final.log`、`menu-values-sdk-verify-final.log`）；来源报告及资源只存忽略目录。原生实际设备交互、原版运行对照和系统页自动迁移仍待完成。

## 批次 28：系统菜单来源解析与映射证据（解析基础交付）

- LSB116 解析器保留稀疏属性位图的真实编号、对象属性、SetProp 三个表达式、函数操作编号与 NotUpdate 标志。声明属性 1-based 与运行时属性 0-based 保持区分；不执行原脚本。
- 解析时记录原始字节 SHA-256，inspect 使用该身份而非二次读取计算。新增 UI 结构数量统计，不泄露原属性值或控件名称。
- LiveNovel 导出增加本地 import-ui.json：保留系统菜单目录中对象/属性修改的源位置及表达式、来源版本/哈希；完整、部分解析和目录不存在有不同状态。未映射对象逐项警告，滑条报告来源量纲的原边界/步长。没有把解析成功改称为 UI 迁移成功。
- 修复本地实际路线测试仅遍历旧 buttons 的遗漏：使用统一 controls 枚举入口和解锁 guard，并防止空回想集合让测试假通过。
- 编译器单测 27 项通过，另有 1 项本地来源测试通过；稀疏高位属性、截断、函数编号、SetProp 编号、未映射/解析错误与来源哈希已覆盖。Clippy 通过；原包 inspect 无解析错误，所有脚本哈希与原字节一致。SDK 重建和独立发行验证通过；实际来源主线及 8 个回想入口均完成并通过快照恢复，主线解锁集合一致。

证据保留在本地忽略目录的 source-ui-* 文件。尚未实现系统菜单布局求值、分支 guard/回调绑定及播放器页面接管；这仍是完整 P0–P6 目标的必要后续，未提交或推送。


批次 28 验收证据：`source-ui-tests-final.log`、`source-ui-clippy-final.log`、`source-ui-real-routes.log`、`source-ui-sdk-final.log`、`source-ui-sdk-verify.log`，以及本地 inventory 与 UI 导出报告。导出包含 218 条来源对象/属性修改、62 条未映射对象诊断、无系统目录解析错误。源码身份在最新 inspect 输出逐个与原字节核对；实际路线测试使用本批前段构建，其导出尚无随后新增的 sources 身份表，该表由中性测试和最新解析身份核对验证，不混称为已重跑实际来源导出。架构与 diff 检查通过。原版实机视觉/交互认证仍未完成。

## 批次 29：来源阅读默认值与采样语音等待（转换闭环）

- Rust 转换器读取 LPB116 已知设置前缀，导出作者默认 BGM/Voice/SE 音量和 Auto 等待；只保留必要设置及来源哈希，不导出项目标题、作者路径或完整配置。未知版本、缺项、非法范围明确失败。
- 根据来源计时回调识别 `固定等待 + 当前剩余语音`，支持临时变量承接的纯加法并拒绝副作用/未知表达式。自动输出 Fixed Auto 延迟和 SampledRemaining 语音边界；旧项目仍使用原字数缩放策略。新增能力分别为 `player.auto-delay-policy.v1`、`text.voice-timer.v1`。
- SampledRemaining 在 Auto 开启且文本就绪时冻结设备剩余时长与静音状态；随后提前结束、音量变化不会重算。尚未就绪则在阅读边界采样。文字速度仅保留来源原值，因单位尚无可靠证据，继续明确报告现有适配，未声称字速保真。
- 实际来源转换得到 1357 页、17 个函数；主线与 8 个回想入口完成并通过快照恢复，检查生成策略与来源默认值一致。原系统菜单仍未自动生成，多个语音通道及原版实机计时对照尚未认证。
- 浏览器首轮 24 项通过、1 项失败，发现运行时把“音频目录尚未加载”误判成“缺少音频时长”。已修复为按资源加载阶段校验，目录准入仍拒绝零时长音频；增加代码先于目录加载的回归。Auto 开启动作也改为当场采样，避免下一 Tick 前音频完成造成竞态。
- 修复后 Core/Player 206 项通过，包含分块加载与输入采样回归。SDK 重建完成；最终浏览器、独立发行与 Clippy 验证结果随后补充。

证据位于忽略目录的 `source-timing-*`：实际来源路线为 `source-timing-real-routes-final.log`，运行时最终测试为 `source-timing-runtime-final.log`。本批未提交或推送，完整实施计划仍未完成。

批次 29 最终验收：浏览器整套 25 项全部通过（`source-timing-browser-final.log`），包含真实音频设备余量采样后提前结束仍按冻结截止点翻页。编译器单测 30 项通过、1 项本地来源测试默认忽略；此前独立执行的来源路线结果另见上述日志。最终 Clippy、独立 SDK 发行验证、架构和 diff 检查通过，分别见 `source-timing-clippy-verified.log`、`source-timing-sdk-verify-final.log`、`source-timing-compiler-verified.log`。SDK 为分块校验/输入采样修复后的 `source-timing-sdk-browser-fix.log` 构建。没有执行原版程序或进行原生硬件对照；系统页、字速单位与余下能力继续保持未完成状态。

## 批次 30：来源界面表达式与分支归一化（转换基础）

- LSB 解析器保留 If/Elseif 条件、While 条件及目标；原来的原始对象/SetProp 报告扩为格式 2，附带正反分支先决条件与循环来源。此处仅描述词法条件，不声称完成全程序可达性分析；静音条件不用于常量排除。
- 新增仅在 Rust 编译器内使用的有界归一化器：消去表达式内临时变量，保留源变量、属性读取、数组索引、有限算术/比较/Min/Max。空值、可归一化值、不支持表达式分开输出；不执行脚本、不调用源函数、不向源环境写入。最多 64 条指令、256 个展开节点、16 KiB 文本，拒绝指数展开、未初始化临时变量、整数溢出、未知函数与隐含写入；未确定的除法舍入和混合类型保持表达式。
- 实际 Auto 转换校验复用归一化器，并核对设置回调的写入倍率与显示倍率、有限赋值及字幕刷新形状。改变单位、插入额外副作用或未知控制流会明确拒绝；不再只靠 Timer 的两项加法推断设置单位。
- 原包 23 个系统菜单脚本、218 条声明/修改完成分析，22 条位于常量排除分支，6 个属性表达式因未支持函数明确保留诊断，来源身份逐个核对，无解析/分支结构错误。报告仍为 parsed_not_lowered，全部未映射对象诊断保留；控件动态尺寸、鼠标锚点、数组与赋值数据流、回调到 NIR 页面绑定继续待实现。
- 编译器套件 82 项通过，2 项私有来源测试默认忽略；来源 UI/计时回调单独测试通过。额外表达式边界 4 项通过，覆盖不整除、零除、极值、未知调用、缓存值不能绕过纯度校验和展开预算。Clippy、架构及 diff 检查通过。
- SDK 重建和独立发行验证通过；使用最终独立二进制重新转换实际原包，生成格式 2 报告及通过加载/编译的工程。仅改变离线解析/转换，未重复浏览器或宣称本批完成原版视觉认证。

证据：忽略目录的 `source-ui-normalization-all-tests.log`、`source-ui-expression-boundaries.log`、`source-ui-normalized-real-final.log`、`source-ui-normalization-clippy-final.log`、`source-ui-normalization-sdk.log`、`source-ui-normalization-sdk-verify.log`、`source-ui-normalized-convert.log` 及本地导出工程。本批未提交或推送；完整 P0–P6 计划继续保持未完成。

## 批次 31：来源布局变量与属性读取保留（转换基础）

- 补齐此前被读过即丢弃的 VarNew、GetProp、WhileInit/WhileLoop 数据：保留变量名、原始类型/作用域字节、初值、属性读回目标及循环更新/目标。没有猜测默认值或把声明直接映射为 NIR 局部状态。
- UI 格式 2 增加 data_flow 列表，使用相同来源位置、分支先决条件、静音/延迟更新标记；Calc 与循环更新区分可识别的单次赋值和 unresolved。声明/属性操作原列表保持独立，共享 4096 条预算。
- 实际来源保留 316 条数据流记录（90 个变量声明、9 个属性读回、181 个计算、36 个循环初始化/更新）；其中赋值类 131 条单次写入、86 条未解析。未解析函数不能被当成无副作用，也没有进行跨调用/跳转的常量传播。
- 编译器单测 39 项通过（2 项私有测试默认忽略），新增字节级类型/作用域/循环目标与截断回归；本地真实来源分析和来源身份核对另行通过。Clippy、架构及 diff 检查通过。SDK 已重建；最终独立二进制原包转换及发行验证结果随后补充。

证据：忽略目录的 source-ui-dataflow-*。仍未生成系统菜单页面；下一步需要解析来源菜单项数组和有界数据流，再连接布局与服务回调。完整计划未完成，本批未提交或推送。

批次 31 最终验收：重建后的独立 novelc 原包转换成功，导出包含 23 个脚本、218 条界面声明/修改与 316 条数据流记录，无分析错误；转换内部加载/编译验证通过。独立 SDK 发行验证通过（`source-ui-dataflow-convert.log`、`source-ui-dataflow-sdk-verify.log`）。未重复运行未改动的播放端测试，不据此声明菜单播放已完成。

## 批次 32：来源菜单项名称/动作配对（转换基础）

- 增加字面量 StringToArray 的有界专用识别，保持其“写入数组”性质，不把它加入纯表达式函数白名单。仅接受已知显式逗号列表、有限非空元素、临时变量结果丢弃；拒绝动态输入、额外写入、未知结果使用及超限项。
- LiveNovel 转换器读取源初始化脚本，验证菜单名称/动作数组的声明、顺序、唯一初始化与配对，新增 import-menu-items.json。来源哈希、版本、写入位置和 NotUpdate 保留；显示文字不决定动作。报告明确标为 declarations_extracted_not_lowered，未据此生成或认证 UI。
- 首次真实来源验证发现初始化声明带 NotUpdate，先前过严规则拒绝了声明提取。现保留标记并允许提取，不推断原运行时的刷新时机。回归覆盖带该标记的声明/写入，显示名称与动作相反、重复/缺项、动态初始化、隐含写入及条目预算。
- 编译器套件 86 项通过（2 项私有测试默认忽略）；修正后的单测 42 项、本地来源 UI/数组分析以及额外两项标记回归通过。Clippy、架构与 diff 检查通过。SDK 已重建，最终独立原包转换和发行验证随后补充。

证据：本地忽略目录 source-menu-items-*；首次来源失败日志保留为 source-menu-items-real.log，修复验证为 source-menu-items-real-final.log。后续仍需把该数组用于有界循环展开、菜单布局与动作 guard/服务绑定；当前提取不证明全程序初始化可达或数组之后未改变。未提交或推送，完整计划仍未完成。

批次 32 最终验收：最终独立二进制重新转换原包成功，菜单项报告包含 22 对名称/动作、2 个数组写入位置，源哈希与原字节一致，顺序索引完整；转换内部项目加载/编译通过。独立 SDK 发行验证通过（source-menu-items-convert.log、source-menu-items-sdk-verify.log）。本批没有新增播放页面，不能用这些通过项替代系统菜单播放验收。

## 批次 33：菜单阅读动作与临时查看剧情（Player/共享投影）

- 根据原选择回调核对语义：Auto/已读跳过退出菜单，隐藏文字等待恢复输入后回原菜单，且能够临时隐藏选择项。新增 `ui.menu-reading.v1`，有限 `reading` 动作包含 auto / skip_read / peek_story；沿用原菜单实例、revision 和控件身份，不引入源脚本运行时。
- Auto/SkipRead 提交时复查上下文与可用性，再关闭菜单并启用模式；重复 Auto 不是反向切换。SkipRead 要求当前文本/意义版本已读，Auto/SkipRead 不允许选择中执行。计时和硬 Gate 继续使用既有阅读策略，零等待遵循原规则。
- PeekStory 保留菜单实例、局部状态、资源、暂停所有者及原阅读模式，仅投影剧情画面；恢复输入被消费并回同一菜单，不推进/选择。选择或没有当前对白时仍可查看画面，故障会退出隐藏以呈现恢复入口。Peek 不写入 Core 快照；宿主屏幕状态与当前投影一致。
- 可用性变化进入菜单 revision；准备/加载/故障、标题/结束、保存确认和其他暂停所有者均拒绝。Peek 时菜单动作/值写入全部拒绝；共享投影使用同一可用性集合。新增源/运行时能力校验及编译器按使用保留能力的合同。
- Rust 首轮相关套件 298 项通过，补选择隐藏后 Player/Engine 131 项通过（新增 1 项；合并范围共 299 项）。Host 68 项、最终 Clippy、Linux 原生包检查、架构及 diff 检查通过。SDK/Schema 重建及最终独立发行检查通过。
- 浏览器定向三项通过，覆盖 Auto 继续、Peek 恢复原菜单、旧动作拒绝、冻结 tick、选择身份不变及既有存档。首轮新增用例误点画布后方的辅助控件，已改为真实画布点击；整套回归另暴露存档/音频用例的时序假设，现等待菜单 revision 刷新及设备实际进度，保持原存档和恢复偏移断言，最终整套结果随后补充。

证据：忽略目录的 menu-reading-* 和 source-menu-dispatch.txt。原菜单布局/guard/回调尚未自动生成；源渐变和焦点恢复细节也未认证，不能据此声称系统菜单迁移已完成。完整 P0–P6 计划仍未完成，未提交或推送。

批次 33 最终验收：最终 SDK 的整套浏览器回归 27 项全部通过（menu-reading-browser-verified.log），包含阅读菜单、选择隐藏恢复、存档及音频偏移断言。测试同步修正仅等待可观察的菜单版本和音频设备进度，没有放宽原断言。此结果验证共享播放器行为，不替代原工程系统菜单的自动转换与视觉对照验收。

## 批次 34：来源菜单组合条件与条目表达式

- 核对来源系统菜单初始化：条目依赖消息框、选择、已读、回想和存取禁止状态，名称数组按动作数组筛选后拼接；菜单不是固定的全量按钮表。因此不能仅按名称/动作对直接生成固定菜单。
- 归一化器保留来源 Or/And/Xor 运算，明确不做布尔化或常量折叠：源运算存在按类型区分的逻辑/按位语义，未证明操作数类型前保持 source_* 符号。新增 Exists、IndexOfStr、Pos、AddDelimiter 的有限只读表达式识别，保留参数顺序，不执行原函数或猜测索引/分隔符结果。
- 原包分析无错误；单次赋值从 131 增至 151，未解析赋值从 86 降至 66。整份报告中未支持表达式记录由 112 降至 64（包含重复的词法 guard，不等同于唯一来源表达式数）。这仍是转换分析，未生成系统菜单页面或认证运行时条件绑定。
- 编译器单测 44 项通过、2 项私有测试默认忽略；真实来源分析单独通过。新增回归覆盖类型相关运算不被错误折叠、对象读取组合条件、未知副作用拒绝、参数顺序/数量及条目拼接赋值。Clippy、架构和 diff 检查通过。

证据：本地忽略目录 source-menu-conditions-*。SDK 与独立转换最终结果随后补充。完整计划仍未完成，未提交或推送。

批次 34 最终验收：SDK 重建、独立发行验证及最终 novelc 原包转换通过。生成的 23 脚本 UI 报告与私有来源测试报告完全一致，无分析错误；转换内部加载/编译通过，保留 1357 个文本页及 17 个函数。本批仅修改离线表达式分析，未重复播放器回归或宣称系统菜单播放验收完成。

## 批次 35：菜单条件绑定与有限纵向排列

- 来源菜单先按状态筛选名称再组成列表，固定绝对坐标会留下空行。新增 `ui.menu-stack.v1` 的有限 Stack 容器，以声明顺序排列可见直接子项，隐藏不占位、禁用保留占位，支持组行、父变换和裁切。固定行高与间隔有明确预算，不引入脚本、动态控件、循环执行或可变文字高度承诺。
- 菜单条件增加 `reading_available {mode,available}`，读取既有 Player 阅读服务权限集合；显隐和启用均可绑定，并继承祖先条件。即使没有 reading 动作也要求 reading/services/state 能力；源和 runtime root 校验、编译器按使用保留。它不是来源变量的隐式别名，来源转换仍需证明条件对应关系。
- 投影和提交使用同一集合，变化沿用 revision；隐藏、禁用条件在控制请求及值写入处复查。补上 Peek 期间值请求的显式拒绝，不能以当前实例/版本绕过暂时隐藏。有限排列同时供绘制、语义区域和命中使用，完整声明控件编号不随显隐改变。
- Rust Compiler/Core/Player/Engine 套件 306 项通过；Host 68 项、Clippy、Linux 原生包检查、架构/diff 检查通过。回归包含权限变化拒绝旧输入、Peek 当前身份值写入拒绝、可见项压缩/禁用占位、缩放与裁切、稳定编号及不合法/超限布局；SDK、Schema 重建及独立 SDK 验证通过。浏览器整套结果随后补充。

证据：本地忽略目录 menu-flow-*。原版系统菜单的文字/hover、背景、完整条件与回调仍未自动生成；本批不能声明原菜单迁移完成。完整 P0–P6 计划仍未完成，未提交或推送。

批次 35 最终验收：最终 SDK 浏览器整套 28 项全部通过（menu-flow-browser-final.log），包含动态显隐排列的实际像素、语义坐标、真实指针与键盘、选择与对白两种上下文的 Peek 恢复。首轮 26 项通过、2 项失败的 menu-flow-browser.log 保留：新增用例误读辅助 DOM 位置，改为检查共享投影的 data-rect；已有反向遮罩曾两次采样超时，改为越过下界后明确断言原 0.48–0.52 范围，并保留观测进度用于错误诊断，没有放宽像素、暂停或存档恢复断言。定向观察及最终两种遮罩均通过；不把软件浏览器结果作为硬件性能或 Windows 实机认证。生产代码在 SDK 构建后未再改变。

## 批次 36：菜单文字按钮

- 新增 `ui.menu-text-button.v1` 的 TextButton：固定标签同时用于作者字体绘制和辅助语义，显式普通/悬停/禁用颜色，禁用样式优先；复用既有变换、裁切、Stack、条件、Profile guard、instance/revision/control 与服务动作，不引入第二条动作路径。
- 字号、标签、颜色和文字批次数量沿用有限预算；所有标签进入作者字体字集与覆盖检查。动态正文绑定暂不允许，避免文字与辅助标签不同步。菜单元素无新增图片依赖，不将文字栅格化为源分辨率图片。
- 源 Program 与 runtime root 均要求声明，编译器按实际使用保留；Rust Compiler/Core/Player/Engine 套件 310 项、Host 68 项、Clippy、Linux 原生包检查及架构/diff 检查通过。回归包括颜色优先级、父透明度/裁切、guard/点击屏障、字体字集以及不合法样式与文字预算。

证据：本地忽略目录 menu-text-*。SDK/Schema 与浏览器最终结果随后补充。来源字体高度/描边、行距、完整布局与回调仍需独立转换规则；未宣称原版系统菜单已自动生成。完整计划仍未完成，未提交或推送。

批次 36 浏览器发现并修复：首轮整套 28 项通过、1 项失败（menu-text-browser.log）。失败发生在鼠标悬停颜色：Web 宿主此前仅向标题页转发移动/离开事件，覆盖菜单根本没有收到悬停输入。现扩至 Title/Menu，离开可恢复普通色；非菜单屏幕清空指针目标缓存。原生端本已使用通用 Engine.hover。保留实际像素断言，并新增鼠标离开后颜色恢复检查。最终 SDK 已重新打包，Host 68 项再次通过，独立发行与浏览器整套复验进行中。

批次 36 最终验收：修复悬停转发后的最终 SDK 浏览器整套 29 项全部通过（menu-text-browser-final.log），文字普通/悬停/离开/禁用颜色实际像素和两种输入均通过，先前菜单、存储、阅读和演出用例未退化。最终独立发行验证通过（menu-text-sdk-verify-final.log）；Schema 与当前格式一致。没有执行原版程序或 Windows 真机认证，来源菜单自动生成仍待完成。

## 批次 37：来源菜单生命周期参数与 Auto 分支校验

- LSB116 不再丢弃 SaveCabinet/LoadCabinet 的稀疏属性、Act 和目标列表；Flip 保留四个前置参数、目标数组及五个后置表达式，明确两个效果参数无长度前缀、116 没有 DifferenceOnly。UI 报告附带原表达式与归一化结果、原位置及条件，仍不执行来源命令。
- 现有自动阅读配置新增标准 Auto 分支体检查：唯一的目标分支，四条未静音/非延迟的直接操作，恢复指定容器、指定菜单背景的退出参数及两次开启赋值。更改参数、目标、赋值，插入操作或重复分支均拒绝，避免只按名称套用标准行为。该检查不证明外围分派、全程序可达性或容器内容；原 UI 退出动画尚未自动映射。
- 编译器完整套件 94 项通过（2 项私有测试默认忽略），真实来源分析单独通过。原包报告仍为 23 脚本、218 条对象/属性操作，数据流由 316 增至 335，新增 9 条 Cabinet 与 10 条 Flip，无分析错误。新增字节级回归覆盖稀疏属性、目标数量、参数顺序、截断；分支回归覆盖生命周期篡改和额外副作用。Clippy、架构及 diff 检查通过。

证据：本地忽略目录 source-menu-lifecycle-*。SDK 和独立原包转换验收随后补充。本批只改变离线编译/转换，没有新增原版菜单页面；完整计划继续未完成，未提交或推送。

批次 37 最终验收：最终独立 novelc 重新转换原包成功，保留 1357 个文本页、17 个函数，转换内部加载/编译通过；导出的生命周期分析报告与真实来源测试报告完全一致。独立 SDK 发行验证通过（source-menu-lifecycle-convert.log、source-menu-lifecycle-sdk-verify.log）。没有改动播放器行为，未重复浏览器检查；原系统菜单自动生成与 UI 退出动画仍未完成。

## 批次 38：来源选择回调入口与分派链校验

- 标准自动阅读配置不再只检查 Auto 分支体：新增有界的入口检查，核对局部动作变量、回想结束特例、按参数标签查名称数组并用同一索引读取动作数组。显示标签不直接决定 NIR 服务。
- 检查互异动作 ID 的单一 If/Elseif/Else 链，以及链后无条件 Exit，拒绝错误动作表、重复条件、断开的条件链、入口尾部/链中/回退后的额外顶层操作和缺失退出。Auto 分支体仍由既有生命周期/赋值校验单独确认；不证明其他分支的实现、全程序数组不变性或容器副作用。
- 初次真实来源检查发现 Else 结构标记带 NotUpdate，过严的边界规则拒绝了原包。修正为仅对 Else 允许该标记，对实际条件、赋值、退出保留限制；回归覆盖入口和回退 Else 的标记，不推断刷新时序等价。首次失败日志保留。
- 编译器完整套件 95 项通过；修正后单测 48 项通过，2 项私有测试默认忽略；真实来源分析单独通过。额外边界、Clippy、SDK 与独立转换最终结果随后补充。

证据：本地忽略目录 source-menu-dispatch-*。原菜单自动生成仍未完成；本批加强当前自动阅读转换的来源约束，不把读取/校验报告当作已迁移菜单。完整计划未完成，未提交或推送。

批次 38 最终验收：补充的入口/回退后额外操作回归、最终 Clippy、架构/diff 检查通过；SDK 重建及独立发行验证通过。最终 novelc 原包转换成功，1357 个文本页、17 个函数保留，内部项目加载/编译通过；导出分析报告与真实来源测试一致（source-menu-dispatch-convert.log、source-menu-dispatch-sdk-verify.log）。本批没有播放器变化，不重复浏览器测试；原系统菜单生成仍未完成。

## 批次 39：转换器生成原系统菜单草稿并实际播放

- LiveNovel `--draft` 在读取原菜单名称／动作表和 Menu 声明后生成 NIR 覆盖页，保留来源标签、顺序、位置、字号／行距及普通／悬停颜色。不增加 NIR 格式或播放器接口。普通导入暂不启用此页。
- 源选择回调入口、Auto 分支体和自动等待策略通过既有校验，菜单声明还必须引用该回调及已知文本变量。只按动作 ID 绑定 Auto 阅读服务，并保留服务可用性检查；其余项目生成灰色静态文字，没有占位动作。原条件、子菜单、截图／容器恢复、200 ms 退出动画仍未迁移，字体、行宽及变暗效果明确标记为近似。
- 输出 `import-menu-preview.json`、不完整标记和错误诊断，状态为 `incomplete_ui_preview`；工程写出后 CLI 非零退出。草稿不是完整兼容声明，不改变 P0–P6 完成标准。
- 编译器完整测试 97 项通过，2 项私有测试默认忽略；补充回调／文本来源限制后两项相关测试再次通过。最终 Clippy、架构及 diff 检查通过。回归覆盖标签与动作 ID 分离、未映射项目无控制、阅读条件、源 BGR 色值、动态／越界样式及错误回调。
- 最终独立 SDK 从原包导出 1357 文本页、17 函数、8 个根菜单项，报告 8 项不完整诊断（7 个未绑定动作和整体限制），返回码 1 且工程已写出。内部编译、Web 发行构建、独立 SDK 验证通过。
- 实际浏览器打开生成菜单：原标签呈现，7 个未绑定项目不存在按钮语义；点击未绑定行不退出菜单或推进剧情；唯一 Auto 按钮可以用键盘启用、恢复 Story 并自动进入下一文本页。无页面异常，截图已检查。未运行原版可执行文件，不声称视觉等价或 Windows 真机通过。

证据：本地忽略目录 source-menu-preview-*，包括最终 SDK、转换／构建／浏览器日志和截图。公共代码仅保留中性回归。下一步仍需迁移原菜单显示条件和其余动作，而非将此草稿当成原菜单完成；完整计划未完成，未提交或推送。

## 批次 40：故事只读导出与已读快进的来源迁移

- 增加 `ui.menu-story.v1`：每页最多 32 个显式 bool/i32 剧情变量别名，条件仅作同类型相等比较。不存在 UI 写剧情通道，不复制快照状态；源／Runtime 根均复查变量、类型、上限及能力。绘制、Stack 排布、语义和提交前校验共享当前投影；值变化刷新 revision，旧输入不可重放。
- Player 回归覆盖投影不泄露未导出变量、反复绘制不写 Core、权威状态改变后即使尚未刷新缓存仍拒绝旧条件、恢复后重新求值和旧凭据失效。增加编译器能力裁剪／错误引用测试及 Runtime 类型、声明、预算校验，Schema 已更新。
- LiveNovel 草稿复用 Auto 的退出过程校验，另核对已读快进的三个标记和两项消息框属性；核对菜单中已读、无选择、非回想的源条件及标签追加表达式。真实来源校验通过；替换已读条件、改变快进属性、增加副作用或改变结构均拒绝。
- 转换器为主线及 8 个回想包装入口写入现有剧情 bool 状态，显式只读导出给菜单。已读快进未读／选择／回想时隐藏，显示时调用现有 SkipRead 服务。只影响 `--draft`，未引入源脚本运行时。隐藏文字分支涉及原组件列表、可见性保存恢复和渐变，仍未自动迁移；其他原菜单动作与完整 P4 Replay 隔离未完成。
- Rust 相关完整套件 319 项通过，3 项私有测试默认忽略；真实来源新增私有检查单独通过。Clippy、Host 68 项、Linux 上原生包编译、架构和 diff 检查通过；SDK 重建及独立发行验证通过。未声称 Windows 真机或原版可执行文件对照通过。
- 新 SDK 从原包生成 1357 文本页、17 函数，内部编译和 Web 发行构建通过；根菜单有 2 个已绑定动作、6 个未绑定动作，报告保持 `incomplete_ui_preview`，7 项诊断，写出工程并非零退出。
- 浏览器整套 30 项全部通过，包括故事变量改变后的显隐、共享布局／命中、陈旧输入拒绝及读档恢复。实际原包生成工程验证：未读快进隐藏；Auto 读过一页后重开主线，快进可见且鼠标激活进入下一文本页；在独立测试 profile 预置回想解锁事实后，经实际标题／回想控件进入，包装入口的上下文为 true，快进隐藏。预置解锁仅用于测试入口，不代表再次验证全路线解锁过程。
- 额外截图检查曾产生文字缺失疑点。对实际 PNG 做逐行像素检查、前后文件哈希比较，确认静止截图完全一致且各行有字；保留绘图缓冲的对照实验也输出同一文件。疑点属于预览判读，未修改渲染器。私有自动化另修正对白等待条件，要求实际 Story 屏幕且下一对白非空，避免将切换中的空状态误认成下一页。

证据：本地忽略目录 menu-story-*，含测试、SDK、发行、来源转换／播放日志与截图。最终原包播放日志为 menu-story-source-browser-final.log / menu-story-source-browser-hover.log；原始失败与补充检查保留。公共用例均为中性材料。完整 P0–P6 计划继续未完成，未提交或推送。

## 批次 41：原隐藏文字分支自动迁移到 PeekStory

- 转换器校验来源隐藏分支的 52 条操作：局部变量及原可见性缓存、指定容器恢复、选择项／父组件处理、ListCompo 输出及数组槽位筛选、三段循环的命令索引跳转、五种恢复输入、两次 Flip 参数、容器保存及最终菜单恢复。源显示条件和消息框存在性声明另行校验。匹配后由原动作 ID 绑定已有 PeekStory 服务，不执行源循环，不新增 NIR 能力。
- 补充 IsDelimiter 的有界符号表示；纯表达式拒绝重复临时目标，避免把数组引用写入误认成纯计算。组件列表输出与槽位清空只在专用副作用形状中接受。新增回归覆盖额外写入、错误输出数组、不同索引／值／目标、循环索引与源码行号混淆及纯表达式中的别名写入。
- 初次真实来源检查发现新校验器误用了 DoEvent 的命令编号；修正为 46，并新增明确拒绝 ClrHist（22）的回归。保留首次失败日志。最终真实来源校验及 6 种分支变异（静音、跳转、缓存、恢复、额外操作、恢复键）全部符合预期。
- `--draft` 现自动生成 Auto、已读快进、隐藏文字三个阅读动作；其余 5 项继续为无动作的静态文字。普通 Story 节点若使用来源隐藏分组的 `#` 前缀，当前草稿明确拒绝，不能在没有 UI 投影时把它留在 Peek 画面。原渐变、任意源 UI 组件及截图／容器内部状态仍未等价迁移，仍是明确不完整草稿。
- 编译器完整套件 104 项通过，3 项私有测试默认忽略；来源契约测试单独通过。Clippy、架构、diff、SDK 重建及独立发行验证通过。本批只修改离线转换／分析，没有改 Player 或宿主运行时，不重复无关后端整套检查。
- 最终独立二进制从原包写出 1357 文本页、17 函数，内部编译及 Web 发行构建通过。报告 `incomplete_ui_preview`，6 项不完整诊断，返回码 1 且工程已写出。
- 实际生成工程浏览器验证：分别用鼠标或键盘触发隐藏，用 Escape、空格、回车、左键、右键恢复；每次都保留同一菜单实例，Core 的位置、交互身份、tick、对白、选择和变量不变。200 ms 观测窗口内故事仍暂停；旧 revision 动作不再隐藏界面。直接分析截图像素确认菜单文字隐藏，页面无异常。

证据：本地忽略目录 menu-peek-*，特别是 menu-peek-source-contract-final.log、menu-peek-source-browser.log、menu-peek-sdk-verify.log。完整计划、剩余菜单动作、原 UI 演出和完整 Replay 等仍未完成；未提交或推送。

## 批次 42：保留来源历史范围和滚动回调参数

- 历史源码检查确认初始显示范围来自消息框高度，滚动条传入 index，滚轮按字号乘三调整位置，并保留页面间隔；现有按条目翻页的 HistoryWindow 无法直接等价替代。连续布局、滚动条部件、原图边框及返回行为继续待实现，不将分析报告视为历史菜单已迁移。
- LSB116 读取器保留此前丢弃的 CallHist 五个参数和 FormatHist 两个参数，保留空格式器与数值零的区别；参数不折算成 NIR 条目单位。UI 分析报告格式升为 3，范围明确扩大到系统菜单和历史回看两个目录，保留原位置、条件、表达式和归一化结果。活动历史调用产生明确的未映射诊断。
- 中性回归覆盖参数顺序、负 index、空格式器、所有截断位置、后续命令对齐、恒假分支和目录边界。编译器完整套件 106 项通过，3 项私有测试默认忽略；真实来源检查单独通过，24 个脚本、242 条对象/属性操作、372 条数据流操作，无分析错误，四处历史调用参数完整保留。Clippy、架构和 diff 检查通过。

证据：本地忽略目录 source-history-*。SDK 及独立来源转换验证随后补充。没有新增播放器能力或可用历史菜单；完整计划仍未完成，未提交或推送。

批次 42 最终验收：SDK 重建与独立发行验证通过；最终独立 novelc 原包转换产出 1357 文本页、17 函数，内部加载／编译通过。历史分析报告与真实来源测试逐项一致；草稿仍为 incomplete_ui_preview、6 项错误、写出后返回码 1。只有离线转换变化，未重复浏览器或原生运行时验收。证据 source-history-convert.log、source-history-sdk-verify.log、source-history-contract-final.log。

## 批次 43：连续历史的真实排版与可见窗口核心

- 新增渲染侧 HistoryLayout，使用实际字体和宽度测量不同长度记录，保留冻结字体／语言；共享不可变历史，维护每条高度索引，复用既有字形 LRU。没有增加 NIR 声明或改变固定行 HistoryWindow，尚未绑定用户菜单。
- 分批测量最多 16 条、通常 64 KiB；超长单条完整处理且单独占一批，避免截断改变排版，不声称严格帧时限。只有准备完成才发布范围，缺字体验证失败可在字体准备后重试；默认定位最新端。滚动后重排保留逻辑字符锚点和行内比例，连续准备期间再次改变尺寸不丢锚点。
- 只返回视口／祖先裁切相交的完整文本 run，首尾部分裁切，64 个可见 run 和总高度预算超出时报错。绘制与测量复用同一 shaping key，滚动不重复测量整段历史。修正完成后重复准备重置滚动位置、锚点恢复额外增加一次未计入批次测量的内部问题，并保留回归。
- Presentation 完整 23 项测试通过；补充小视口缩放约束后，9 项连续布局核心回归再次通过，涵盖长文换行、部分可见、1000 条分批和缓存上限、空历史、缺字体重试、字体／宽度重排、可见预算、超长单条和重复调用。布局使用投影后的单位，缩小视口时不错误套用作者字号下限。Clippy、架构和 diff 检查通过。

这仍是连续历史菜单的底层实现，页面会话、实例化输入校验、滚动条、两宿主接入及原菜单迁移待完成；不以 CPU 测试替代实际播放或原版体验认证。完整计划未完成，未提交或推送。证据：本地忽略目录 history-flow-*。SDK 验证随后补充。

批次 43 最终验收：小视口／极小字号和大间隔造成浮点高度丢失的专项回归通过，最终 Clippy、SDK 重建和独立发行验证通过（history-flow-scale-final.log、history-flow-clippy-verified.log、history-flow-sdk-verified.log、history-flow-sdk-verify-final.log）。本批核心尚未被 Player 菜单调用，未声称浏览器或原包连续历史已可用；原菜单仍保持此前三个阅读动作及五个未绑定项目。

## 批次 44：连续历史窗口接入 NIR、Player 与两宿主输入

- 增加独立 `ui.menu-history-flow.v1` 和 HistoryFlow 元素，保留旧固定行历史语义。每页最多一个连续窗口，显式字号、行高、记录间距、滚轮／翻页步长和可见预算；与现有文本共同计入 64 run 上限，并按最小读者字号比例校验密度。源与 Runtime 根复查能力／预算，编译器裁剪未使用能力，Schema 更新。
- Player 按页面实例冻结共享历史投影，重复 UiModel 不复制全文；关闭或切换实例释放投影。Engine 使用上一批实际文字测量核心分批准备，准备期间继续 UI 调度但不推进 Story；完成后才发布范围。隐藏／禁用／父级完全裁切时没有滚动输入，加载提示也遵守父级与视口裁切。
- 专用滚动请求携带菜单 instance/revision、窗口 ID 和布局版本。每次滚动或重排更新布局版本；提交前重建当前投影校验，拒绝旧页面／旧布局、非法方向／比例及准备中的请求。Web 滚轮、垂直拖动、PageUp/PageDown 与原生滚轮／翻页键进入同一服务，不改变剧情或存档。图片滚动条部件尚未加入。
- 中性回归验证 1000 条只冻结一次、分批范围发布、可见预算、重排、关闭释放、隐藏／禁用及旧输入拒绝；编译器验证能力裁剪，发行后 Runtime 再次拒绝缺能力或坏预算。初次发行测试被新中文提示的字体覆盖检查阻止，已改用现有字集能覆盖的自然提示。集成测试另修正了字号变化后未等待资源准备完成便索引滚动视图的问题；初始失败日志保留。
- 最终相关 Rust 套件合计 357 项通过（Compiler 107、Core 84、Format 4、Player 138、Presentation 24；Engine 无独立测试），3 项私有来源测试默认忽略。本批未改变导入器，不重复私有来源解析。Clippy、Host 68 项、Linux 上原生端编译、架构及 diff 检查通过。
- SDK 重建、独立发行验证通过；浏览器整套 31 项通过。新增实际 40 条记录用例验证默认最新位置、滚轮和翻页后的实际图像变化、字号重排、视口外滚轮不生效、关闭重开后的旧实例拒绝；整个菜单操作期间 Core 位置、tick、交互、变量及历史数量保持不变，无页面异常。

证据：本地忽略目录 history-flow-*，主要为 history-flow-rust.log、history-flow-runtime-final.log、history-flow-runtime-clippy-final.log、history-flow-browser.log、history-flow-integrated-sdk-final.log、history-flow-integrated-sdk-verify.log。新契约见 MENU-HISTORY-FLOW-SEMANTICS.md。未声称 Windows 真机验收；原历史页的图片滚动条、原图边框、分页间隔／格式器及来源回调自动映射仍未完成。原转换预览仍为三个已绑定阅读动作、五个未绑定项目；完整计划继续实施，未提交或推送。

## 批次 45：作者图片历史滚动条与持续拖动

- 增加 `ui.menu-history-scrollbar.v1`：同页连续历史窗口的轨道、固定高度滑块及两端箭头，四部件各自显式普通／悬停／按下／禁用图片。所有状态图片进入准备和引用闭包，页面最多一个滚动条，部件计入既有绘制预算；源和 Runtime 根复查能力、目标与几何，编译器裁剪未使用能力。
- 共享 Engine 手势在按下时保存抓取偏移，移动时持续提交有界比例；箭头使用字号缩放的 line_step，轨道使用窗口 page_step。当前只有单次箭头／轨道按下，没有按住自动重复契约。每次移动刷新布局版本，外部滚动、重排、裁切／几何改变、页面／会话切换取消旧捕获；绘制、裁切、命中和覆盖控件共享顺序。
- Web 指针捕获与原生鼠标进入同一手势层，捕获后的松键由宿主消费，不能在关闭菜单后触发新页面。失焦、后台、指针取消与 Web 设备恢复取消捕获；轨道以垂直 slider 提供值与范围，Up／Down、Home／End 使用同一服务。焦点保留忽略更新版本，提交仍严格复查。
- Rust 完整相关套件 361 项、Host 69 项通过；新增回归覆盖 1000 条历史、抓取偏移、持续拖动、图片状态、裁切／覆盖屏障、绘制顺序、键盘方向、错误控件和旧布局／会话拒绝，以及能力、几何和所有状态资源类型。首次 Clippy 检出新增枚举变体过大，改为部件图片状态的内部间接存储，序列化保持相同；最终 Clippy（含两个宿主与 SDK 工具）、架构及 diff 检查通过。

SDK、Schema、独立发行及浏览器实际像素／输入验收随后补充。本批没有完成来源状态条拆分或原历史页自动迁移；原转换预览继续三个已绑定阅读动作、五个未绑定项目。源码图片尺寸不足以证明状态顺序，不以通用能力代替来源兼容结论。Windows 真机仍待验收，完整 P0–P6 计划继续实施，未提交或推送。证据：本地忽略目录 history-scrollbar-*；契约见 MENU-HISTORY-SCROLLBAR-SEMANTICS.md。

批次 45 Web／SDK 最终验收：新 SDK 重建、Schema 与两个模板的契约文档更新、独立发行验证通过。图片状态内部间接存储后，三项 Player 回归与编译器的资产闭包／Runtime 校验专项再次通过。浏览器整套 32 项通过（history-scrollbar-browser-final.log），包括四类图片实际像素、按下不跳动、松键前持续移动、控件外捕获及端点钳制、箭头与轨道不同步长、稳定 slider 焦点和键盘方向、取消／缩放／关闭后的旧拖动拒绝、旧页面请求拒绝及 Core 不变。首轮 31 项通过、1 项失败：新增用例在 resize 后只等待旧投影仍满足的就绪状态，随后错误比较布局版本；改为等待实际投影宽度与准备完成，未放宽原断言或改变生产代码。首次日志与错误上下文保留，最终截图 history-scrollbar-half.png 已检查。

来源下一步还需有界子页返回：来源历史 Escape 删除历史容器并恢复原系统菜单；当前通用 Close 返回 Story，尚不等价。另需核实状态图拆分、来源 ScrollbarHeight／整体几何和 CallHist 范围单位。不能仅添加根菜单链接便声称原历史页完成。Windows 专属窗口／鼠标路径已通过 x86_64-pc-windows-gnu 目标的 cargo check（history-scrollbar-windows-check.log），补足 Linux 公共包检查不覆盖 cfg(windows) 的缺口；设备实测继续待验收。

## 批次 46：有界子页导航、父页局部值与逐层返回

- 增加 `ui.menu-navigation.v1`，作者控件可 push 静态菜单或 back。每个上下文最多八个父页，只保存静态 ID 和既有有界局部值；不留存父页媒体、历史全文或页面控制器。导航 ID 最多 128 字节，未使用导航的旧菜单不增加这一限制。源／Runtime 根复查能力和目标，overlay 可达闭包同时遍历 replace／push 并禁止未隔离 Entry；编译器裁剪未使用能力。
- 子页从初始局部值开始；返回恢复父页局部值并分配新实例，重新投影服务权限，旧父页／子页输入失效。根 back 和达到上限的 push 同时在投影与提交端禁用。没有可绕过菜单控件验证的宿主 push/back 请求；旧 menu 动作仍只替换当前顶页。
- 共享 Close 在有父页时返回一层，保持 Story 暂停；保存确认先消费一次 Close。标题子页支持 Escape／右键，临时系统覆盖与标题链分别有界保存；新游戏／恢复等会话切换清除对应链。离开系统子页取消准备并释放图片，准备中或准备失败时可返回，旧资源回执不能恢复已退出页。连续历史重新进入仍使用该控件已有定位政策，不保存任意页面快照。
- 接入网页 Escape、原生 Escape、共享右键及返回标签。网页 `host_state()` 补齐菜单深度和历史滚动条元数据：此前滚动条元数据只在诊断 state 中，网页的悬停缓存例外未生效；新增轨道移向滑块的实际像素回归，避免共享语义节点吞掉部件悬停变化。
- 最终 Rust 完整相关套件 367 项、Host 69 项通过；Clippy（含两宿主及 SDK 工具）、实际 Windows 目标编译、架构和 diff 检查通过。新增回归覆盖父局部值恢复、新实例、深度上限、原始请求绕过、标题／覆盖链、准备中及失败后返回、迟到资源、旧 ID 兼容、能力裁剪和 Runtime 引用复查。

SDK 已重建，Schema 已更新；独立发行验证与浏览器整套 33 项进行中。中性导航用例验证标题返回、父局部值像素、历史子页操作、旧输入及 Core 不变；全部验证结果随后补充。来源历史 Escape 的父页恢复已有表达能力，但转换器尚未生成原历史页，原预览仍为三个已绑定阅读动作、五个未绑定项目。P4–P6 与完整来源映射继续待实施，Windows 设备待验收，完整计划未完成；未提交或推送。证据：本地忽略目录 menu-navigation-*；契约见 MENU-NAVIGATION-SEMANTICS.md。

批次 46 最终验收：配套 SDK、Schema 和独立发行验证通过（menu-navigation-sdk.log、menu-navigation-sdk-verify.log）；浏览器整套 33 项一次通过（menu-navigation-browser.log）。新增导航用例实际验证标题 Escape／右键、鼠标／键盘 push／back、父局部值的文字像素恢复、新实例和旧父／子输入拒绝、历史滚动后返回时 Core 不变，以及根菜单退出重开时局部值重置。滚动条轨道转向滑块的实际颜色回归也通过。来源转换规则未改变，不重复私有来源转换，也不将上述中性验收当作原历史页已迁移。

## 批次 47：来源历史页自动生成与只读可用性

- LiveNovel `--draft` 校验已知 116 历史页面的完整命令结构、格式器注册、初始样式、选择／滚动／滚轮回调及 Escape 删除目标。LSB 读取器保留 Label 和 Delete 的原内容，回归覆盖截断与后续命令对齐。源码单位依据已注册 FormatHist 的官方帮助语义核实为排版高度，Source ScrollbarHeight 只指轨道高度，不能直接作为包含箭头的整个控件高度。
- 按已核实的横向状态顺序自动拆分轨道 2 状态、滑块 3 状态和箭头 6 状态；原尺寸图片作为 NIR 部件，八片边框按自然像素平铺。生成连续窗口的几何、字号／行距、滚轮／翻页步长和原图子页；来源动作 ID 绑定 push，Escape 使用逐层返回。原始分析报告保持 parsed_not_lowered；只有已校验并生成的该页诊断改为 E_IMPORT_UI_DRAFT_ADAPTED。
- 新增 `ui.menu-history-availability.v1`：当前历史是否非空的只读条件，不冻结全文、不要求历史窗口。显隐、Stack 排布、命中和提交复查同一事实，改变时刷新 revision。根历史项另检查消息框可用性；中性回归覆盖无记录、禁用／隐藏、权限变更前缓存、旧输入和 Core 不变。
- 最终 Compiler 完整套件 113 项通过，4 项私有来源测试默认忽略；真实历史页及根菜单契约单独通过，增加错误历史谓词、静音历史保留设置等拒绝检查。相关 Core、Format、Player、Presentation、Engine 完整套件通过，Host 69 项、Clippy（含两宿主与 SDK 工具）、实际 Windows 目标编译、架构及 diff 检查通过。

- 实际来源验收发现两处通用问题：DOM 焦点通知排队时立即按 Home 会丢失；默认导航与网页发行入口占用原上箭头。数值键现按当前控件／页面身份解析，不等待焦点通知，同时拒绝数字 ID 被其他页复用。发行入口只在菜单根页显示。新增 `ui.menu-chrome.v1` 静态声明，来源历史页自动生成 builtin_navigation=false；旧页默认开启且序列化省略缺省值，Escape／右键和准备失败返回继续有效。中性回归验证同轮焦点与 Home、边缘箭头、旧默认导航及源／Runtime 缺能力拒绝。
- 最终相关 Rust 整套 374 项、Host 69 项、Clippy（两宿主与 SDK 工具）、Windows 目标编译、架构及 diff 检查通过。SDK、Schema 和两个模板更新，独立发行验证通过；最终浏览器整套 33 项通过，包含立即 Home、作者导航政策及根页发行入口可见性检查。
- 最终独立二进制重新从原包生成 1357 文本页、17 函数、四个绑定动作及四个未映射根菜单项。历史页的导航政策来自转换器生成，未手改导出主题。草稿保持 incomplete_ui_preview、六项不完整错误和 CLI 返回码 1；工程已写出且发行构建通过。报告、不完整标记及历史页限制一致，19 条对应来源诊断改为明确的草稿适配警告。
- 独立 Pillow GAL 解码器逐像素验证 11 张状态 PNG 与完整平铺 RGBA 边框一致。最终实际生成工程从主线积累 30 条历史后，验证原几何、96 像素滚轮、40 像素箭头、361 像素轨道翻页、滑块持续拖动和首尾夹取；截图文字随滚动变化。立即 Home、两端原箭头、Escape／右键返回父页、新实例与旧父／子／布局请求拒绝全部通过。菜单操作期间观测的 Core 位置、tick、交互、变量、对白、选择及历史数保持不变，无页面异常；返回父页后继续暂停，根页 Close 返回同一剧情。

本批第四个根菜单动作已完成自动转换链路及实际播放验收，历史仍为明确不完整草稿：NIR 字体与有界记录替代来源格式器；scenario-page 间隔、按格式器页数保留、字体阴影／描边、动态样式、箭头按住重复及精确轨道拉伸尚未等价迁移。原可执行文件对照和 Windows 设备待验收，完整 P0–P6 计划未完成；未提交或推送。最终证据：本地忽略目录 source-history-rust-verified.log、source-history-clippy-verified.log、source-history-windows-verified.log、source-history-sdk-verified.log、source-history-sdk-verify-verified.log、source-history-browser-verified.log、source-history-convert-verified.log、source-history-source-verified.log、source-history-pixel-evidence.json 与 source-history-page-browser-evidence.json。此前失败记录保留；契约见 MENU-HISTORY-FLOW-SEMANTICS.md、MENU-SERVICES-SEMANTICS.md 与 IMPORT.md。

## 批次 48：菜单页面效果（P4.1）

- 新增 `ui.menu-effects.v1`：页面边界声明式效果——进入/关闭 MenuTransition（可选一次性音效 + ≤2 秒渐隐）、接受提交点击音效、前台域循环页面音乐（可配总线和 0–4 增益）。`deny_unknown_fields` 与 `E_VIEW_EFFECTS` 校验；编译器按实际声明保留能力并连带菜单服务。
- 所有权与静默准备：进入效果属主为 `(页面 ID, 菜单实例)`，覆盖页只在自身 prepared stamp 完整后触发，标题闭包页随启动准备就绪；换页/离开面立即停旧页音乐并退休旧页效果，revision 变化不重播。点击音效只在通过实例/版本/守卫校验的提交上播放，过期请求与恢复投影不触发。
- 关闭事务：声明关闭效果时退出变为有限事务——关闭音效立即播放、旧输入锁定、渐隐保持页面与暂停，淡出完成才提交退出（含阅读模式以原始交互身份重放开关、取消的槽位恢复重启激活准备）；故障/紧急路径可立即退出，进入故障保持标题静默。
- 时间与音频域：效果状态属前台域，渐隐先取 ForegroundClockToken（MAX_TASKS 上限，完成/取消释放；令牌不可得时进入全不透明立即呈现、关闭立即提交，不产生隐形死页）。一次性音效与音乐走 `foreground_ui` 域 `(域, 会话, 任务)` 寻址；会话重置随宿主 AudioReset 终止全部效果声音，循环音乐从不进入等待集合，未知前台音频失败不构成故障。`reduced_motion` 保留声音、跳过全部渐隐。
- 设计缺口修复：效果音频资产并入页面准备闭包——新增 `ImageMenu::prepared_assets`（图片 ∪ 效果资产），标题闭包准备、覆盖页 `prepare_active_menu`、准备完成 stamp 子集检查与菜单留存全部改用；否则宿主 `playVoice` 只能播已解码缓冲、留存修剪会立即删掉刚准备的音频（浏览器实测 E_AUDIO_BUFFER 全静默）。
- 呈现与状态：UiModel.menu_opacity 全页绘制透明度乘数（作者菜单分支），engine state() 暴露 `menu_opacity`；MenuEffectsState 会话瞬态，不入故事快照。
- Rust 契约覆盖：标题/覆盖静默准备空窗、每 (id,instance) 单次进入、音乐随页（换页停旧起新）、关闭声音/锁输入/延迟退出/音乐停止、点击仅在受提交、reduced_motion（有声无渐隐无令牌）、ForegroundUi AudioEnded 按 (task,session)、未知失败不故障、会话重置全清、进入故障标题静默。最终 `cargo xtask test` 33 套件通过（batch-48-xtask-test.log）。
- SDK 重构后浏览器验收通过（batch-48-browser-menu-effects.log）：标题进入恰一次（1 音效 + 1 循环音乐），200ms 稳定后不再重播；Start 后会话重置终止 UI 域声音、Story 域 BGM 起；覆盖页就绪后进入（累计 4 起）且稳定；过期 data-action 不增点击音效、真实提交恰 +1；Escape 关闭立即响铃、渐隐中保持 Menu 与音乐停止数不变、完成后才切 Story 并停音乐，透明度全程 0→1 可观测。无页面错误。

P4.1 完成；Replay 事务（P4.2）开始。证据：reports/nir-next/batch-48-xtask-test.log、batch-48-xtask-sdk.log、batch-48-browser-menu-effects.log。契约见 MENU-EFFECTS-SEMANTICS.md、CAPABILITIES.md 与 TIME-DOMAINS.md。完整 P0–P6 计划未完成；未提交或推送。

## 批次 49：Replay 事务（P4.2）

- 新增 `ui.replay.v1`：图片菜单 `replay` 控件动作（具名函数 + 可选 `requires` 解锁键）与活动相限定的 `exit_replay`。三相事务 entering/active/returning（engine `state().replay` 暴露），启动时冻结原会话描述——Core 快照、检查点、屏幕/返回屏、菜单面、菜单页与局部值/父链/挂起标题页（FrozenMenu 含 instance/revision）、auto/skip；候选 Core 以 `Core::new_at` 创建，入口块只在候选内以独立预算推进到第一个激活/内容屏障，声音、痕迹与 Profile 意图在切换前不存在。
- 切换与返回：候选媒体（`Purpose::Replay`，激活号取候选待定 Cue id，`CoreInput::Prepared` 与之一致）准备完成后切换——会话自增、音频重置、检查点重记、菜单面关闭；回想函数 outcome 结束或手动 `exit_replay` 进入 returning，冻结会话作为 Restore 候选重新验证/准备，提交后在新会话与菜单实例/版本下接回（unfreeze 同时提升 instance 和 revision，冻结前菜单输入全部过期，页面效果按新实例重放）。回标题/NewGame 显式放弃整个事务。
- 隔离不变量：单一活动 Core；活动回想（active/returning）内 `profile_merge` 不落玩家 Profile 也不发出 `PersistProfile`；Save/Export 活动期拒绝，Load/Import 存在任何回想事务即拒绝，Rollback 仅无事务或 active 后允许（returning 中回退会覆盖返回候选）；嵌套 replay 与存储控件动作在 `resolve_menu_control` 和动作派发双重复查即死亡，`exit_replay` 非活动相幂等；已有准备/恢复候选/槽位读取进行中时新 replay 静默忽略。菜单投影按 `replay_active` 门控保存/读取/回想/退出控件。
- 失败与恢复：entering 中资源失败保留冻结页与原会话，Retry 重启候选自身媒体；准入失败整事务即刻作废（候选与冻结态同弃，诊断剥除 Retry——重试已无可提交候选），释放后重新点击从头开始；设备丢失经 DeviceReady 按候选自身资源恢复；取消准备不再丢弃 entering 中的回想（重试/设备恢复路径复用同一候选），真正的放弃只在标题/NewGame 分支显式清除。设计自查修复四项：Replay 准备激活号 0 与候选 pending 不符、cancel_preparation 误毁 entering 事务、Returning 中 Rollback 覆盖返回候选、槽位读取与回想并发竞态。
- 资源：候选媒体与冻结会话联合准入（`active.retain` 释放不再交集的冻结资产，重叠回想在 LIMIT-1 仍可进入；含未保留资产的回想在零余量下明确 E_BUDGET）；入口块内容屏障按 `ContentPurpose::ReplayEntry` 获取；`E_THEME_ENTRY` 覆盖 replay 动作；源/Runtime 双重校验 `ui.replay.v1`（uses_replay 时编译器保留）。
- Rust 契约 12 项（nir-player replay_tests）：锁定未解锁拒绝、双击仅一次进入、outcome 返回（Profile 隔离 + 返回中 ExitReplay 幂等 + 冻结位置/检查点/新菜单实例 + 冻结前权威过期）、手动退出返回、无活动回想时退出拒绝、活动相内嵌套入口/存储派发复查死亡、资源失败保留冻结页并可 Retry、准入失败整事务作废无 Retry 且后续新点击可用、标题/NewGame 放弃、入口期设备丢失恢复候选、旧 entry 不回归。最终 `cargo xtask test` 33 套件全部通过（batch-49-xtask-test.log）、SDK 重建通过（batch-49-xtask-sdk.log）。
- 浏览器验收（端口 4221，`replay.spec.js` 2 项 + 整套 36 项全过，batch-49-browser-replay.log / batch-49-browser-full.log）：锁定控件禁用且伪造当前授权动作同样死亡；IndexedDB 播种 "seen" 解锁；同一 data-action 双投递仅一次进入，entering 期间冻结页保持屏幕；active 后回想正文与冻结正文不同、覆盖层仅提供退出；outcome 后返回原页原正文、控制件恢复可用而冻结前点击不再起效；手动 `Exit replay` 经覆盖控件完整返回。无页面错误。

P4.2 完成；P5（Sequence/ParallelAll 组合）待实施。证据：reports/nir-next/batch-49-xtask-test.log、batch-49-xtask-sdk.log、batch-49-browser-replay.log、batch-49-browser-full.log。契约见 REPLAY-SEMANTICS.md、CAPABILITIES.md。完整 P0–P6 计划未完成；未提交或推送。

## 批次 50：Sequence/ParallelAll 有限组合（P5.1）

- 新增 `task.compose.v1` 与 `Effect::Sequence`/`Effect::ParallelAll`：组合是一个自主任务，主 VM 停驻于等待、选项或内容屏障时链仍自行前进——以“主 VM 等对白时，另一条先位移再淡出链仍前进”的样例证明仅靠 Activate/Await（单一等待槽）不可编译。子项经与 Cue 提交共用的 `commit_effect` 路径派生：作用域继承、所有权检查、AudioStart 意图与句柄注册完全一致，但派生发生在子项自己开始的时刻，序列后项捕获前项结束后的当前值。
- 执行语义：结果归并失败 > 取消 > 完成（ParallelAll 取消仍在运行的兄弟）；已完成副作用不回滚；链被 Finish 控制时运行中子项按各自 FinishPolicy 落终值、未派生子项永不执行；`end`/作用域退出照常级联。零时长子项在同一提交的追赶轮内连锁完成，每次派生消耗一个执行预算单位（预算耗尽跨步续跑、work_used 可见），追赶不收敛显式 E_LIMIT 故障，无限零时长循环不可表达。
- 校验（源与 Runtime 根共用）：子项作用域必须继承组合 scope、禁用 StagePresent/Dialogue 子项、全树 ID 唯一、并发写冲突（序列位置可改写前项地址，Parallel 兄弟及链外并发效果不可）、嵌套深度 ≤ 8、单 Cue 叶子 ≤ MAX_TASKS 256、停止子项只指向同模块顶层音频任务。子项 ID 不进入故事名字索引：Await/TaskControl/DialogueVoice 按子项名寻址直接 E_TASK，组合只能作为整体被等待或控制。
- 快照与恢复：children/cursor 恒等、已派生子项逐项匹配声明、序列至多一个运行中子项、Finished 链无待办；损坏即拒绝。链中途存档读档/回退后已完成音频不重启、运行中音频只按保存的故事偏移重发一次 AudioStart（续播非重播）、Tween 的 elapsed/captured 原样恢复。
- 编译器盲区修复：能力裁剪（audio.gain.v1/audio.stop.v1/tween.target.v1）此前只扫描 Cue 顶层效果，嵌套子项会被误裁——新增 `Effect::effect_tree_any` 全树判定；`runtime_roots` 与激活配方 `cue_assets` 改用 `collect_audio_assets` 遍历子项音频资产，否则嵌套音频不进准备闭包。端到端由浏览器夹具构建失败（E_CAPABILITY tween.target.v1）发现并验证。
- Core 契约 8 项（compose_contract.rs）：VM 等对白时链自主前进且对白正常收束、零时长连锁与预算可见、子项失败保留已完成副作用、ParallelAll 归并与兄弟取消、Finish/Cancel 整链、链中途快照恢复零重播、源校验八类拒绝、恢复校验拒绝损坏组合。Player 协调 2 项：并行链中途本地变量（affection=5）改变后存读档、链中途检查点回退，均断言恰好一次携带故事偏移的 AudioStart、冻结 Tween 值保留、链恰完成一次、结局到达（batch-50-core.log、batch-50-player.log）。
- 浏览器验收（端口 4222，compose.spec.js 1 项 + 整套通过）：真实 Web 播放器中主 VM 等待对白揭示时，先淡面板（background_opacity→0.2）再淡正文（text_opacity→0.35）的序列链自行按序走完两段，无页面错误（batch-50-browser-compose.log）。SDK 已用修复后的编译器重建。

P5.1 完成；P5.2（故事交互与类型化结果）待实施。证据：reports/nir-next/batch-50-xtask-test.log、batch-50-xtask-sdk.log、batch-50-core.log、batch-50-player.log、batch-50-browser-compose.log。契约见 COMPOSE-SEMANTICS.md、CAPABILITIES.md。完整 P0–P6 计划未完成；未提交或推送。

## 批次 51：故事交互与类型化结果（P5.2）

- 新增 `story.typed-result.v1`：Interact 扩展 `result`（目标变量）与 `on_cancel`（取消路径块），选项以 `value` 携带常量值。值由唯一 VM 写入——宿主只报告选项 id，`OfferedChoice.values` 是呈现快照，提交以声明为准；超时按显式选择 default 行提交其声明值。取消是完整输入（last_input、Checkpoint、`input:cancel` 痕迹），跳转 `on_cancel` 且不写任何值；未声明取消路径的交互是模态的，派发点复查即拒绝。
- 语义选择游标：类型化交互携带 `OfferedChoice.selected`（进入交互时为 default 行，缺省首个启用行），进入 Core 快照并随恢复返回；悬停与键盘焦点保持呈现瞬态、永不进快照。`SelectChoice` 是对挂起交互的观察——无输入身份、不推进 last_input、无检查点、序列号被忽略，未知/陈旧/禁用一律忽略。引擎把落在选项行上的键盘焦点经 `sync_focus_selection` 以普通动作路径（`AppEvent::Action`，序列 0）同步为游标观察。
- 校验（源与 Runtime 共用 + 恢复权威校验）：目标变量存在（E_VARIABLE）、逐选项带值且类型匹配（E_TYPE）、on_cancel 命名同函数块（E_BLOCK）、能力门控 E_CAPABILITY（编译器按实际使用裁剪）；恢复时 values 逐项等于声明、selected 命名存活启用行、result/on_cancel 与规范终结符一致、普通交互不携带结果状态，损坏即拒绝。恢复的交互获得全新交互身份（含会话轮换）。
- E_INFINITE_WAIT 推广：定义的效果树内任何位置出现循环音频叶子都使自然 Finished 不可达（序列停在该叶子、ParallelAll 永远等不齐），包级与源级两站点对整棵效果树扫描——直接 Await 循环音频、sequence/parallel_all 内嵌循环子项一律诊断，非循环音频保持可等待。
- 宿主集成：`UiAction::SelectChoice`/`CancelChoice`；Web host 的 Escape 在可取消交互上优先取消，紧凑状态暴露 `choice_cancellable`；桌面 Escape 路径与呈现层取消出口据同一状态渲染。
- Core 契约 8 项（typed_result_contract.rs）：提交先写值后分支、普通交互不携带结果状态、取消无写入且陈旧取消拒绝、游标观察与快照恢复零进度、超时提交 default 值、恢复七类篡改拒绝、源校验五类拒绝、循环音频三形态 E_INFINITE_WAIT（含非循环反例）。Player 协调 3 项：交互中途存读档恢复挂起交互与游标（恢复身份轮换用 assert_ne 断言）、类型化提交后回退撤销写入并重新挂起、取消分支无写入且未声明路径时拒绝（batch-51-core.log、batch-51-player.log）。
- 浏览器验收（端口 4223，typed-result.spec.js 4 项 + 整套通过）：选项声明值经 Switch 驱动不同对白、键盘焦点移动语义游标且 sequence/interaction 不变、Escape 经声明路径取消且 picked 保持 0、交互中途存读档恢复游标后照常提交（batch-51-browser-typed.log）。SDK 已重建并核对夹具 wasm 与 dist/sdk 哈希一致。
- P5 关卡核对：并行链中途局部状态改变后存读档/回退不重播（批次 50+51 测试覆盖）；无限循环媒体与 All 的不可完成组合被诊断（本批 E_INFINITE_WAIT）。“第二来源案例复用相同核心”未满足——LiveNovel 导入器尚无 Interact/选项到类型化结果核心的映射（import/*.rs 无相关引用），留待导入器批次。

P5.2 完成；P5 关卡三条中“第二来源复用相同核心”未满足（导入器无 Interact 映射，留待后续批次）。证据：reports/nir-next/batch-51-xtask-test.log、batch-51-xtask-sdk.log、batch-51-core.log、batch-51-player.log、batch-51-browser-typed.log。契约见 TYPED-RESULT-SEMANTICS.md、CAPABILITIES.md。完整 P0–P6 计划未完成；未提交或推送。

## 批次 52：第二来源选择映射复用类型化结果核心（P5 收口）

- 证据先行：解码标准选择系统三页并据此定型约定——`選択.lsb` 回调把所选项文本 `@ParamStr[0]` 写入 `選択値`（`選択番号 = @ParamStr[1]`）；`■選択実行.lsb` 清空结果变量后按 `@ParamStr` 数组参数（位置/皮肤/音效/倒计时配置，选项文本即 `@ParamStr[0]`）创建 kind 25 Menu 对象并等待关闭；调用方以连续条件跳转对 `選択値` 与单个字符串字面量做操作 12 比较分发（标题分发实测：はじめから/つづきから/回想三条 Jump）。选项的显示文本、分发字面量与提交值三者同源。
- 导入器降级（livenovel.rs）：识别"无条件调用選択メニュー执行页 + 紧随的選択値 字面量分发链 + 链尾 Exit（后继不可达）"，整个调用点合成一个类型化 Interact——每个字面量声明为选项，`value` 为该字符串（等于源回调提交值），`result` 指向导入器声明的 `選択値` 字符串变量（初值空串，经 fragment `variables` 合入 Program），分支目标为对应标签处续块。提交、写入、快照、恢复、回退全部走批次 51 的同一核心路径，无导入器私有分支；能力 `story.typed-result.v1` 由编译器按实际使用裁剪机制自动声明。
- 路线图行走：主线行走从线性单指针改为队列驱动图行走（entries 映射命令位置到续块，合流跳转合并为同一续块不复制内容，回到自身可达路径的跳转按路线循环拒绝），剧情内选择可嵌套；主线函数改用显式块 id 组装，剧集函数、回想包装、文本/媒体管线不变。此前用于寻找新游戏路线的标题分发现有 `selection_dispatch` 统一谓词匹配，不再各写一份。
- 严格拒绝（E_IMPORT_CHOICE）：条件调用选择执行页、调用后缺失分发链、链不足两项、选项文本重复、链尾非 Exit。菜单皮肤、悬停/选择音效、倒计时与对齐参数不迁移；报告状态保持 `converted_with_adaptations`，coverage 按选择位数注明"Branching … typed choice site(s)"并新增差异警告。IMPORT.md 与 TYPED-RESULT-SEMANTICS.md 记录映射契约与证据。
- 测试：livenovel 单元 6 项新增/重整（selection_dispatch 单字面量谓词、选择点降级为类型化交互并经 nir_format 反序列化校验、合流分支合并、路线循环与四类畸形拒绝）；真实语料回归（RJ061378 全量转换 1357 页 17 函数不变，main+8 回想在 VM 中走完、快照恢复、解锁断言）确认线性路径行为不变（batch-52-corpus.log）。
- P5 关卡核对（三条全部满足）：并行链中途局部状态改变后存读档/回退不重播（批次 50/51 测试覆盖）；"第二来源案例复用相同核心"（本批：LiveNovel 選択メニュー约定降级到 story.typed-result.v1 核心，import/*.rs 现有 Interact 映射）；无限循环媒体与 All 的不可完成组合被诊断（批次 51 E_INFINITE_WAIT）。

P5 收口；P4 遗留（消息/UI 根遮罩与通用目标）与 P6（映射级别/完整路线认证/发行清单）待后续批次。证据：reports/nir-next/batch-52-corpus.log 及本批五项门禁日志（batch-52-xtask-test.log、batch-52-clippy.log、batch-52-architecture.log、batch-52-xtask-sdk.log、batch-52-browser-full.log）。契约见 IMPORT.md、TYPED-RESULT-SEMANTICS.md。完整 P0–P6 计划未完成；未提交或推送。

## 批次 53：ImportReport 映射级别与显式近似接受（P6.1）

- 报告格式升为 2：新增 `mappings` 账本与 `approximate` 计数。每条记录规则 ID、行为级别（exact/adapted/approximate/unsupported）、证据类（documented/decoded-source）、源版本（LSB116/LPB116/LPM106/GAL105/106）、规范行为一句话、依赖的目标能力与近似位置/未决项；公共记录不含私有路径或正文。聚合状态由账本推导（converted → converted_with_adaptations → converted_with_approximations；unsupported 维持 blocked/草稿契约），不再是硬编码字符串。
- 近似是兼容声明而非可忽略 warning：`enforce_acceptance` 在工程与报告发布之后执行，任何未按规则 ID 显式接受的 approximate 规则以 `E_IMPORT_APPROXIMATE` 失败并逐个点名；拼错的 ID 无法静默通过（真实规则仍被点名）。`--draft` 保持自己的 incomplete 契约、跳过该门禁。
- LiveNovel 账本 13 条（choice 位置按实际位数出现）：settings exact；startup/system-services/title-menu/stage.wipe/auto-policy/media.image/media.audio/replay/story.choice adapted（分别标注 ui.menu-elements.v1、ui.replay.v1、story.typed-result.v1、audio.gain.v1、player.auto-delay-policy.v1+text.voice-timer.v1 等依赖能力）；menu-sfx/text.reveal/textbox.fade 三条 approximate——原版实机与跨后端尚未作为证据类出现，未决项保留在近似说明与保真警告。通用 LSB 路径账本：control-flow exact、text adapted、阻塞时汇总 unsupported。
- CLI 新增 `--accept-approximate <ids>`（逗号分隔）。中性测试：账本完整性（级别/证据域、exact 与 adapted 不携带近似说明、规则唯一、无私有路径）、三条 approximate 恰为文档所列、状态推导、接受门禁（部分接受点名缺失规则、typo 不放行、全接受通过、draft 跳过）；通用路径两用例补映射断言（converted_with_adaptations + lsb.unsupported-commands）。
- 本批只改离线导入器/CLI 与报告格式，不触及 Player/运行时；按批次 37/38/42 先例不重复浏览器整套验收。真实语料回归（RJ061378）经 ignored 测试（其 options 预接受三条规则）与独立 CLI 双向验证：不带门禁标志转换在发布后以 E_IMPORT_APPROXIMATE 退出并列出三规则，带 `--accept-approximate` 全列表成功且 1357 页/17 函数不变。

P6.1 完成；P6 余下完整路线认证与能力发行清单，P4 遗留（消息/UI 根遮罩与通用目标）待后续批次。证据：reports/nir-next/batch-53-xtask-test.log、batch-53-clippy.log、batch-53-architecture.log、batch-53-xtask-sdk.log、batch-53-verify-sdk.log、batch-53-corpus.log、batch-53-corpus-gate.log。契约见 IMPORT.md（映射级别、证据与近似接受一节）。完整 P0–P6 计划未完成；未提交或推送。

## 批次 54：完整路线认证（P6.2）

- 新增 Player 级实包认证 `real_livenovel_player_certifies_full_routes`（import/certify.rs，ignored，需 NIR_IMPORT_SOURCE/NIR_IMPORT_OUT；tests.rs 的 options/sdk 提升为 pub(super) 供其复用）。与既有 VM 直驱回归不同，全部路线经共享 Player 的输入路由、阅读策略、菜单控件解析与存档事务。四个场景：A）Auto 自动阅读走完主线至结果，解锁集与导入器在菜单控件上声明的 lm.replay.* 完全一致，read: 标记落盘，结果清除自动阅读。B0）全新档案经真实 MenuControl 分发带锁回想入口被拒——不切换会话、不离开标题、停留在原菜单。B）经作者菜单控件（标题菜单 Menu 控件 → 回想菜单 Entry 控件）进入全部回想入口，每条读完经 return-to-title 结果回到回想菜单且无故障。C）已读主线在按住快进下整线快进至结果；无 auto/skip 时 tick 不推进。D）隐藏（默认继续政策）剧情时钟继续、恢复不推进；菜单暂停/关闭恢复同页；演出中保存 → 前进 → 回退（Continue 释放 restored 暂停）→ 槽位读档精确恢复保存页与阅读位置（text_id、span/cluster、gate/awaiting 位逐一相等）→ 走完全程。
- 认证 harness 承担宿主的音频结束职责：每次 tick 前对全部 Running 非循环 Audio 任务注入 AudioEnded（宿主契约同 coordination.rs）。缺了它，语音 WAV 作为 Running 任务永久钉住解码资产，内存账本必然耗尽——属 harness 缺陷而非产品缺陷，在本测试内修复。
- 读档断言认证页面与阅读位置的精确往返（text_id、span/cluster 揭示进度、gate/awaiting 位在菜单暂停下随存档冻结、读档后逐一相等），不比对 interaction 令牌：Core 快照恢复按设计为恢复中的对白/选择重铸交互身份（vm.rs restore："restored interactions receive fresh identities, in addition to the host epoch change"），陈旧输入拒绝由会话纪元承担；比对保存时的令牌值是错误断言（全语料首两轮运行先后在 15≠17 与揭示位上误报）。
- 产品修复：MEMORY_LEDGER_LIMIT 128→256 MiB（nir-player 常量 + 注释）。全语料多次运行在 episode7/b000680 确定性达到 ~130 MiB 峰值（105.76 MiB 已钉 + 24.16 MiB 待入：场景交叉淡入需新旧 cue 同驻，加常驻字体），admission 剪枝正确、无泄漏——是上限过紧而非泄漏。256 MiB 仍为硬上界，防失控租约；replay.rs 与 media_tests 的测试常量改为引用同一常量（其 admission 测试压到零余量，必须与真实上限一致）。
- 本批触及 Player 运行时 → 按先例恢复浏览器整套验收（先 cargo xtask sdk 重建 dist/novelc）。nir-compiler 测试 dev-dependencies 加入 nir-player/nir-presentation。

P6.2 完成；P6 余下能力发行清单，P4 遗留（P2.4 消息/UI 根遮罩转场与 P1.2 UI/音频通用目标）待后续批次。证据：reports/nir-next/batch-54-corpus-certify.log、batch-54-xtask-test.log、batch-54-xtask-sdk.log、batch-54-verify-sdk.log、batch-54-clippy.log、batch-54-architecture.log、batch-54-browser.log。契约见 IMPORT.md（完整 Player 级路线认证一节）。完整 P0–P6 计划未完成；未提交或推送。

## 批次 55：能力发行清单（P6.3）

- docs/CAPABILITIES.md 新增「能力发行清单」：代码 `CAPABILITIES` 全部 43 项逐一登记执行证据（Rust 测试）、恢复证据（快照/存读档/回退，或「无运行态（仅校验）」并给出校验测试）、Web 后端（浏览器 E2E 规格的实际执行）与 Windows 原生状态。Windows 原生全部如实记为「待验证」——公共 Rust 测试不覆盖 cfg(windows) 原生路径，不据此声明实机通过。个别能力的浏览器覆盖为间接，备注如实标注而不当作直接断言：ui.menu-chrome.v1 的 builtin_navigation=false 页实际运行但无按钮缺失直接断言；media.webp.v1/media.mp3.v1 经 fixture 产物实际解码播放，无容器级直接断言；task.compose.v1 浏览器仅直接覆盖 sequence，parallel_all 经 Rust 组合/协调测试。
- 新增 scripts/verify_capabilities.py 并接入 `cargo xtask test`（check_architecture.py 之后）：代码能力表与清单必须一一对应（缺行、幽灵行分别点名），引用的 .rs/.js 证据路径必须存在，Windows 列只允许「待验证/✓」。负例已验证：缺行报 "advertised in code but no ledger row"、非法 Windows 值报 "Windows column must be 待验证 or ✓"、幽灵行报 "ledger row for unknown capability"。
- 能力→证据映射由两路独立检索汇总（Rust 测试侧与浏览器规格侧），引用路径全部经文件存在性核对；基线能力（v0.1.0）与 NIR-NEXT 批次交付在备注中区分，批次号取自实施进度文档。
- 本批只改文档/脚本/xtask 测试挂接，不触及 Player/格式/编译器运行时；dist/novelc 自批次 54 门禁后未变，按批次 53 先例不重复浏览器整套验收。

P6 三项（映射账本、完整路线认证、能力发行清单）全部交付；P4 遗留（P2.4 消息/UI 根遮罩转场与 P1.2 UI/音频通用目标）与 P0 三个完整示例/版本上限矩阵待后续批次。证据：reports/nir-next/batch-55-xtask-test.log、batch-55-xtask-sdk.log、batch-55-verify-sdk.log、batch-55-clippy.log、batch-55-architecture.log。契约见 docs/CAPABILITIES.md（能力发行清单一节）。完整 P0–P6 计划未完成；未提交或推送。

## 批次 56：消息根窗口转场（P2.4）

- `dialogue_visibility` 增加可选 `transition` 与 `duration_us`（0 < duration_us ≤ 60 秒），缺省与旧两字段操作仍是立即翻转；样式化操作要求 `text.window-transition.v1`，源校验（nir-core validate：能力、样式、时长门）与 runtime 加载双侧执行，nir-format 契约测试覆盖旧文件缺省读取与样式化序列化往返。
- Core：`WindowReveal { style, to_visible, from_coverage, started_us, duration_us }` 跟随 Story 时钟并加入 needs_clock；提交的 `dialogue_hidden` 只在截止时刻翻转，反向同款操作以打断时刻覆盖度为新起点，与已提交状态一致的同款操作立即提交，旧立即翻转会中断在飞揭示；恢复对样式/遮罩做结构校验（invalid window reveal / invalid window reveal mask），遮罩必须是 Image 类并进入准备闭包与恢复资产。契约测试 window_reveal_contract.rs 5 项（截止提交、打断捕获、冗余立即提交、立即翻转中断、校验/恢复门）。
- Player/Presentation/Renderer：dissolve 把覆盖度乘进消息框背景与文字透明度（HUD 不参与）；擦除/遮罩把消息框项剥离为窗口根并在原位留下覆盖全表面的哨兵四边形（@window），复用舞台转场的双输入混合路径——隐藏方向把已渲染窗口放在 source 侧由覆盖度擦除，窗口根本身每帧重绘以保持正文揭示与外观动画。协调测试 76 项含：投影跟随时钟并截止提交、reduced_motion 跳过动画直接提交、飞行中存读档续播、遮罩只取一次并钉住时钟、dissolve 仅折叠窗口项、wipe 哨兵几何与前后绘制序不变。TopUp 遮罩请求会并入状态资产，宿主须喂满整个扣留请求。
- 引擎状态桥 `state.window` 暴露进度（null 或 0..1）。浏览器规格 tests/nir-next/window.spec.js 在固体红开场场景 + 1×1 绿色窗口背景 fixture（4216 端口）上采样：领先侧擦除为场景色、尾侧保持纯绿、软边覆盖度与进度一致；暂停冻结、飞行中存读档恢复同进度、恢复后继续清空。fixture 修正过一处：复用 wipe() 场景改写会连带注入舞台擦除转场，导致背景本身处于蓝红擦除中途——改为仅改写 `station` 场景，开场静态呈纯红。
- 导入器：MESON/MESOFF 非零渐隐毫秒映射为等时长 dissolve 窗口揭示（fade_sites 计数入账本），零渐隐保持立即翻转；账本规则 `livenovel.textbox.fade` 由 approximate 升为 adapted（存在渐隐位点时依赖 text.window-transition.v1），批次 53 记录的近似清单相应收窄为 menu-sfx/text.reveal 两条。发行侧遮罩随窗口揭示进入媒体根（window_transition_masks_join_the_release_roots_and_capability）。文档同步：CAPABILITIES.md（能力段落 + 发行清单行）、STAGE-TRANSITION-SEMANTICS.md（消息根一节）、TWEEN-SEMANTICS.md、IMPORT.md。

P2.4 的消息根半边交付；UI 菜单页根（MenuTransition 样式、ui.menu-transition.v1）与 P1.2 UI/音频通用目标、P0 三示例/版本上限矩阵待后续批次。证据：reports/nir-next/batch-56-*.log。契约见 STAGE-TRANSITION-SEMANTICS.md（消息根一节）。完整 P0–P6 计划未完成；未提交或推送。

## 批次 57：菜单页面根空间揭示（P2.4）

- MenuTransition（进入/关闭转场）增加可选 `style`，复用舞台 StageTransition 的 wipe（方向 + 软边）与 mask（图片遮罩 + 通道）；dissolve 或未声明样式保持旧的整层 alpha 渐隐路径（menu_opacity），不需新能力。`MenuEffects::uses_transition()` 判定空间样式使用；mask 的遮罩是 Image 类页面资产，经 `MenuEffects::mask_assets()` 并入 `ImageMenu::image_assets()`（Image 类校验、准备与留存），不从音效闭包取用。nir-format 契约测试覆盖旧文件缺省读取、样式化序列化往返、遮罩闭包与能力语义。
- 新能力 `ui.menu-transition.v1`：源（nir-core validate）与 runtime（from_runtime）双侧拒绝「使用空间样式而无能力」（E_CAPABILITY），仅声明未使用合法（编译器裁剪，menu_transition_capability_follows_spatial_style_usage）；样式边界时长仍须 0 < fade_us ≤ 2 秒（E_VIEW_EFFECTS），遮罩非 Image 资产以 E_THEME_ASSET 拒绝。契约测试 menu_transition_contract.rs 4 项。
- Player：空间揭示持有 ForegroundClockToken，期间输入锁定、阅读暂停；进入等页面准备落定后才起播；关闭立即播放音效并锁定，退出延迟至擦除完成提交；reduced_motion 抑制呈现不抑制音效。协调/菜单测试新增 7 项：无能力拒绝、进入分流页根跟随前台时钟（@menu 哨兵、页资产只出现在页根、menu_paint 清空、进度推进与收尾复位）、关闭反向合成与延迟退出、reduced_motion、无样式停留共享面、连续历史滚动条拼接随页面吸收进页根。
- Presentation/Renderer：`divert_menu_page()` 后置通道把页面四边形与页面文本分流到离屏页根（历史拼接吸收在 menu_page_range.to + 4），页面在共享菜单面清空；渲染器双根合成复用舞台转场混合路径（进入 bind(底层,页面)、关闭 bind(页面,底层)），期间 menu_opacity 恒为 1。
- 引擎状态桥 `state.menu_transition` 暴露进度（null 或 0..1），与 `state.window` 同型。浏览器规格 tests/nir-next/menu-wipe.spec.js 在固体红剧情场景 + 黑色菜单背景 fixture（4224 端口）上采样：进入中途左侧为页面、右侧为冻结帧，完成覆盖全帧；关闭中途反向（页面保留在左、帧回归在右），延迟退出期间 screen 保持 Menu 且 paused，完成后回 Story 且 paused=false；全程 menu_opacity=1。文档同步：CAPABILITIES.md（能力段落 + 发行清单行）、MENU-EFFECTS-SEMANTICS.md、TIME-DOMAINS.md。
- 转场完成脉冲（产品缺陷修复）：页面渐隐的收尾发生在纯时钟 tick 内（clock-only，无工作即不置脏），且同一刻 ForegroundClockToken 释放、宿主帧循环停摆，16 帧安全阀不再触发——落定帧可能永不重投影（批次 48 的 alpha 渐隐即已潜伏，空间揭示使其可见）。修复：Player 以 `ui_visual_pulse` 标记「本 tick 渐隐由有到无」的离散视觉变化（`take_ui_visual_pulse()` 取走），Engine 在 `pump()` 末尾将其并入 `state_dirty`；回归测试 a_completed_reveal_pulses_the_view_before_the_clock_token_releases 断言中途 tick 无脉冲、收尾 tick 恰好一次脉冲。浏览器规格连过 4 次后全量 43 项绿。

P2.4 的菜单页根半边交付；P1.2 UI/音频通用目标与 P0 三示例/版本上限矩阵待后续批次。证据：reports/nir-next/batch-57-*.log。契约见 MENU-EFFECTS-SEMANTICS.md（空间揭示样式一节）。完整 P0–P6 计划未完成；未提交或推送。

## 批次 58：实例增益补间与菜单元素进入动画（P1.2 收口）

- Part A——AudioInstance 增益目标（`audio.gain-tween.v1`）：`tween.target.v1` 的目标联合增加 `audio_instance { task, property: gain }`，把一个已建立 Audio 实例的 0–1 包络乘子在有限时长内线性补间到 `to`（0–1）。包络叠加在事件 gain × 总线音量之上，不预乘 PCM、不改变播放位置与生命周期——声音始终 Running，设备侧只是包络节点上的一条线性 ramp；只允许 `easing = linear`（设备每次只渲染一条线性段，非线性行为由作者分段表达）。目标必须是同模块的另一音频任务，与 `audio_stop` 共享互斥的包络所有权（同 Cue 验证期 E_OWNERSHIP，运行中冲突在原子提交回滚）。补间完成提交终点并 Finished，声音留在该音量；Cancel 提交设备时钟当前值（设备领先剧情时钟时以设备值为准）；飞行中存读档保存剩余段、恢复续播，设备包络检查点（owner/elapsed）同样适用于补间 owner。Core 契约（audio_contract.rs）覆盖线性推进、取消提交设备值、与停止的所有权互斥、飞行中存读档剩余段与伪造 owner 拒绝。
- Part A 实机链路：vm.rs 的包络段路径（原属 AudioStop）推广到补间 owner——`CoreIntent::AudioEnvelope` → Player `AppCommand::AudioEnvelope` → Web host 在语音专属包络 GainNode 上 `cancelScheduledValues/setValueAtTime/linearRampToValueAtTime`（与事件 gain、总线节点分离）。浏览器规格 tests/nir-next/audio-gain-tween.spec.js（端口 4226 fixture：循环铃声 + 2 s 补间到 0.25）审计设备上恰有一条指向 0.25 的排定 ramp、中途值单调、终点收敛且循环声源存活。AudioParam 绝不跨越 evaluate 边界（结构化克隆会丢弃活动节点）——一律在页内按下标解引用。
- Part B——ViewElement 进入动画（`ui.menu-element-tween.v1`）：`MenuEffects.elements` 至多 128 条元素轨道（opacity/scale/offset_x/offset_y，from 落在属性界限内，时长 (0, 2 秒]、延迟 ≤ 2 秒，同一元素同一属性单轨），随进入边界在共享前台时钟上启动：延迟期保持 from，随后推进到落定值——opacity/scale 落定到元素作者值、偏移落定到 0，完成的轨道被丢弃，落定投影与未声明不可区分。投影层在布局前把轨道值覆盖到元素 Node（父级变换随之传播，透明度乘进颜色 alpha）；页面本身仍走共享菜单面（不建离屏页根、不叠加页面渐隐，`menu_opacity` 恒为 1）。换页逐页重启，离开菜单面清空轨道并释放时钟令牌（与页面渐隐共用同一枚 ForegroundClockToken，全部轨道落定才释放），最后一条轨道的落定 tick 复用批次 57 的 `ui_visual_pulse` 标记视图脏；reduced_motion 抑制动画不抑制音效。状态瞬态、不入故事快照；使用而无能力在源与 Runtime 校验以 E_CAPABILITY 拒绝；引擎状态面暴露 `menu_element_progress`（无动画为 null）。
- Part B 实机验证：浏览器规格 tests/nir-next/menu-element-tween.spec.js（端口 4225 fixture：slide 元素 offset_x from=1400、2 s 滑入 + 不动画锚点）以 `hidden(true)` 冻结两域于飞行中途，像素采样断言滑动列被覆盖而未到达列为背景、锚点恒定，同时 `menu_transition === null`、`menu_opacity === 1` 证明走的是共享菜单面而非页根；解冻落定后滑动列回到背景、元素列回到作者位置；键盘关闭返回剧情无错误。开发期修正一处落定谓词写反（mid-flight 几何本已证明动画正确）。
- 文档同步：AUDIO-SEMANTICS.md（实例增益补间一节）、MENU-EFFECTS-SEMANTICS.md（元素进入动画一节）、CAPABILITIES.md（两条能力段落与发行清单行，能力数 45→47）、TWEEN-SEMANTICS.md、TIME-DOMAINS.md（元素轨道共用前台时钟租约）。
- 门禁：工作区 Rust 测试 49 套全绿、Clippy、依赖架构（18 包）、SDK 重建（batch-58-partb-tests/clippy/arch/sdk.log；Core 契约另见 batch-58-core.log）；浏览器整套 44 项通过后新增两条规格分别单独复跑通过（batch-58-partb-browser.log、batch-58-partb-browser-element.log、batch-58-parta-browser-gain.log）；`cargo xtask test` 常设门禁含 verify_capabilities.py 对 47 项能力的一一对应核对（batch-58-xtask-test.log）。

P1.2 计划所列四个目标域（SceneNode/DialogueRoot/ViewElement/AudioInstance）全部接入共享求值；P4 的通用补间目标随之交付。遗留：P2.4/P4 来源映射（导入器菜单转场与元素动画）、P2.3 来源样式/字体映射、P0 三示例与版本上限矩阵。Windows 原生一律待验证。证据：reports/nir-next/batch-58-*.log。契约见 AUDIO-SEMANTICS.md（实例增益补间）、MENU-EFFECTS-SEMANTICS.md（元素进入动画）。完整 P0–P6 计划未完成；未提交或推送。

## 批次 59：LiveNovel 菜单音效映射（P4 来源映射）

- 导入器新增三个严格按引擎固定约定提取的助手（缺失约定静默为 None，识别但畸形以 E_IMPORT_LIVENOVEL 拒绝）：`title_select_sound` 读引导脚本中对 `プレビューメニュー\■選択実行.lsb` 调用的第 6 参（选择音，空串为无音）；`replay_select_sound` 读 `サムネイル・マウス処理.lsb` 中 `選択` 标签之下的 kind-42 "SE" 对象文件字段；`replay_bgm` 把 `■関数.lsb` 的 `BGM再生` 标签区间解析为行号范围，只接受落在该区间、单参数字面量的调用——匹配约定而非「任意带声音路径的调用」。路径反斜杠统一归一为 `/`，经 `self.sound(path, 1.)` 进入既有音频资产管线（Ogg 经 lewton 转 WAV）。
- 映射落点：标题页与回想网格页的 `ImageMenu.effects` 各获得 click 一次性音效（Player 侧 `play_ui_sound` 走 Sfx 总线，音量随 live.lpb 的 StatusSEVolume → sfx_volume 默认值），回想页另获得循环页面音乐（`MenuMusic { bus: Bgm, gain: 1 }`，音量随 StatusBGMVolume）；两个页面各自计入 menu_sounds。`ImageMenu::image_assets()`/`MenuEffects::assets()` 已把 click/music 并入页面媒体闭包，编译器按 `uses_effects` 自动保留 `ui.menu-effects.v1`——无需播放器侧改动。
- 账本拆分：`livenovel.menu-sfx` 由 approximate 升为 adapted（存在映射位点时依赖 ui.menu-effects.v1，行为句记录 {menu_sounds} 与音量语义）；悬停音效与动画光标无 NIR 对应机制，新设近似规则 `livenovel.menu-hover` 显式点名（「NIR 菜单无悬停驱动音频或自定义指针光标」），沿用批次 53 起的原则——近似必须显式接受而非静默警告。近似清单收窄为 menu-hover/text.reveal 两条；`--accept-approximate` 示例与 `APPROXIMATE_RULES` 测试常量同步更新。
- 实证：真实语料转换（1357 页、main + 8 条回想全路线含快照恢复）通过（batch-59-corpus-test.log）；重建发行 CLI 后，无接受时以 E_IMPORT_APPROXIMATE 点名 [livenovel.menu-hover, livenovel.text.reveal] 退出且工程已写出，带新 ID 接受时成功——theme.toml 中 title.click 与 replay.click/music（bgm 总线）及 import-media.json 中 tm2_switch002.wav/BGM054mama.ogg（gain 1.0）逐项核对（batch-59-cli-gate.log、batch-59-sdk-novelc.log、batch-59-verify-sdk.log）；助手单测覆盖空/缺参/非字面量/二义调用、悬停-only 处理器、非 SE 对象、区间外调用与多参数调用。门禁：`cargo xtask test`（47 项能力）、Clippy、依赖架构（batch-59-xtask-test/clippy/architecture.log）。仅改导入器，按批次 53/55 先例不跑浏览器套件。
- 文档同步：IMPORT.md（近似规则清单与菜单音效描述、保真边界句）、CAPABILITIES.md（ui.menu-effects.v1 段落与外部引擎导入行）、MENU-EFFECTS-SEMANTICS.md（不包含清单改为指向导入映射）。

P4 的来源效果映射半边交付；悬停音效/动画光标保持显式近似，来源菜单转场/元素动画映射（批次 57/58 能力的导入侧）、P2.3 来源字体样式与字速单位、P0 三示例与版本上限矩阵待后续批次。Windows 原生一律待验证。证据：reports/nir-next/batch-59-*.log。契约见 MENU-EFFECTS-SEMANTICS.md。完整 P0–P6 计划未完成；未提交或推送。
