# Story 与 Foreground UI 时间域

此文描述分域时钟、暂停和音频路由的实现基础。自定义页面、页面音乐绑定、Close 动画和 Replay 返回事务仍依赖后续阶段，不能据此声明这些能力已可编写。

## 所有权与推进

Story 继续由唯一 Core 持有任务与逻辑时间。Foreground UI 是 Player 的瞬态时间域，不运行第二份剧情，不进入故事快照，也不能成为 Core Await 的目标。

Player 分开维护两个域的暂停令牌。系统菜单只增加 Story 的 menu 暂停；后台、设备丢失和事件队列溢出同时暂停两域。一个原因的解除不能释放其他原因，也不能释放同名的另一枚外部令牌。旧 acquire_pause 接口保持 Story 语义；新 acquire_domain_pause 显式选择域。

Player 对实际分发的 Tick 推进未暂停的前台时间，不再将经过时间截成 250 ms。计算量仍受每轮工作预算限制；Core 未处理完的剧情时间作为独立续算事件保留，不能重复推进前台时间。宿主向 u32 接口单次分发至多 u32::MAX 微秒，剩余经过时间保留待下一轮处理。无活动 UI 动画时不维持无意义的帧循环；页面效果 owner 使用 ForegroundClockToken 请求时钟，并在完成/取消/销毁时释放。请求数量以 MAX_TASKS 为上限。前台有时钟需求时，菜单下仍可调度 Tick，但不会因此推进已暂停的 Core。token 与时间在 Player 侧，不持久化为剧情任务；菜单页面渐隐（`ui.menu-effects.v1`）是首个生产消费者：进入/关闭渐隐开始前取得令牌，令牌不可得时进入以全不透明立即呈现、关闭立即提交，不产生永不推进的隐形页面；后续页面实现必须同样负责其生命周期。

Web 宿主通过 tick_domains 分别发送 Story/Foreground UI 经过时间。进入菜单、菜单内和退出菜单的一轮保留 UI 时间，只丢弃剧情边界时间；后台与会话替换丢弃两域旧时间。输入尚待处理时保留各自累计值，u32 分批不会重复计算。旧 tick 接口仍把同一 delta 交给两域，由 Player 各域暂停令牌过滤。

音频包络使用所属域的设备采样时钟，而不是浏览器墙上时间；暂停相应设备时钟同时冻结采样位置与淡出进度。

## 播放身份与宿主契约

AudioStart/AudioEnvelope/AudioStop 使用 `(domain, session, task)` 定位实例。AudioPause/AudioReset 必须明确 domain。合法域是 story、foreground_ui，未知域拒绝；事件总线和任意对象寻址不属于本接口。

所有 Core 音频意图由 Player 标记 story。完成/失败回调携带域，foreground_ui 回调不能用相同数字任务号结束或报错某个 Story 任务。当前尚无声明式 UI 音频 owner；未拥有的前台回调被忽略，后续页面媒体接入时由 Player 的页面 owner 接收，不能透传 Core。

Web 使用两个 AudioContext，解码缓冲可共享。手势解锁两个设备上下文，但不能解除暂停所有者；延迟解锁完成后重新检查当前暂停状态。每条包络在对应 context.currentTime 上调度，域内 reset 只停止所属声音。资源留存仍计入所有活动声音。

Windows 使用同一输出设备、按完整实例键保存 Sink，Pause/Reset 仅作用于匹配域的 Sink；逐采样包络随 Sink 的播放状态冻结。Windows 原生路径已同步修改，但 Linux 公共测试不等于 Windows 编译或实机验收。

宿主命令与 WASM 必须来自同一 SDK。新增方法 audio_ended_in/audio_failed_in 明确域；旧同名无后缀方法继续作为 Story 包装。此改动不新增作者 NIR 能力声明，也不改变源/内容包/快照版本；SDK 内容身份会变化，不支持任意混用新宿主与旧运行时。

## 验证范围

Rust 测试检查菜单下的独立推进、外部令牌与后台原因交错、时钟请求限额和释放、跨域回调隔离。Host 测试检查两域暂停、迟到解锁、关闭及未知域。

浏览器测试通过真实两个 AudioContext 与原创静音缓冲检查菜单下 Story 冻结而前台采样时间继续，后台暂停两域，返回前台保留菜单对 Story 的暂停。该测试验证宿主路由，不伪装为已经实现声明式菜单音乐。实际结果见 NIR-NEXT-PROGRESS.md。
