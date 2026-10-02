# NIR-NEXT P0 基线账本（N01–N10 / R01–R06）

对应实施计划 §4「P0：先固定需要实现什么」。本账本把方案需求固化为可核对的需求 ID、已修复回归的保留测试、验收样例清单与素材重建程序；完成证据 = 能力账本（docs/CAPABILITIES.md 发行清单）+ 本文档的版本/限额指针 + fixture 清单 + 基线测试记录。

边界沿用计划 §1：公共仓库只存中性样例；原包、原版行为记录及含来源信息的报告保留本地忽略目录。本文档引用的第三方游戏仅存在于本地实验，不进入仓库。

## 1. 需求账本 N01–N10

需求 ID 对应实施计划 §3 阶段表与 §5–§9 各阶段的验收条款。状态取值：已交付（附批次与常设证据载体）、待验证（Windows 原生一律待验证）、待批次 N（未交付）。

| ID | 需求（计划出处） | 状态 | 验收载体 |
| --- | --- | --- | --- |
| N01 | 任务终态与播放实例：四种 TaskState 之上的终态原因（自然结束/显式完成/取消/替换/scope 退出/失败）、实例代次、重复 stop 与迟到 ended 幂等（P1.1） | 已交付 | audio_contract.rs 等契约测试；Await 分支优先级回归；Windows 原生待验证 |
| N02 | 类型化目标与共享求值：SceneNode、DialogueRoot、ViewElement、AudioInstance 四域经白名单验证接入同一求值机制（P1.2） | 已交付（批次 56/58） | tween/window/menu-element 契约与协调测试；`audio.gain-tween.v1`、`ui.menu-element-tween.v1` 发行清单行 |
| N03 | 分域时钟与暂停：Story 与前台 UI 逻辑时钟、嵌套暂停不互相解除、恢复锚点不重放（P1.3） | 已交付 | TIME-DOMAINS.md 契约；协调测试；浏览器规格；Windows 原生待验证 |
| N04 | 阅读边界与输入：源交互/源页/布局视口页三分、页内 Gate 后继续同一对白、字速与 Auto 等待为独立偏好、held skip 与隐藏（P2.2） | 已交付 | READING-SEMANTICS.md；fixtures A（Gate×2 + 固定 Auto 策略）；浏览器输入规格；Windows 原生待验证 |
| N05 | 消息窗口与文字目标：样式化显隐（dissolve/wipe）、阴影、FontPlan 映射、来源字速单位（P2.3） | 已交付（来源字速批次 60） | 批次 56 窗口揭示契约；fixture A 窗口显隐；来源字体面保持显式近似（livenovel.text.font） |
| N06 | 遮罩转场：舞台场景根、消息窗口根、菜单页根的方向 wipe 与图片 mask（P2.4） | 已交付（批次 56/57） | STAGE-TRANSITION-SEMANTICS.md / MENU-EFFECTS-SEMANTICS.md；window.spec.js、menu-wipe.spec.js；fixtures A/B 使用侧；硬件 WebGPU 未认证 |
| N07 | 音频事件：事件 gain、带时长停止、实例增益补间、总线乘算（P2.1） | 已交付 | AUDIO-SEMANTICS.md；audio-gain-tween.spec.js；fixture A（BGM 0.7 + 50 ms 语音淡出停止）；Windows 原生待验证 |
| N08 | 有限页面组合与系统服务：stack 菜单、值控件（Range/Toggle）、有界历史窗口、存档槽与确认令牌（P3） | 已交付；player 级验收样例待批次 62 | MENU-*.md 系列语义文档；批次 36–47 协调与浏览器测试；fixture C（页签设置/存档/历史）待批次 62 |
| N09 | 页面效果与隔离回想：进入/关闭边界效果、页面音乐、元素进入动画、Replay 冻结-切换-返回事务与 Profile 守卫（P4） | 已交付（批次 48–49/56–58） | MENU-EFFECTS-SEMANTICS.md / REPLAY-SEMANTICS.md；fixture B（锁定回想 + 页面效果/转场/元素动画）；Windows 原生待验证 |
| N10 | 兼容迁移与发行：类型化结果复用、映射级别账本与显式近似门禁、完整路线认证、能力发行清单（P5/P6） | 已交付（批次 50–55）；第二来源认证待批次 63 | IMPORT.md / CAPABILITIES.md；import-report 格式 2；verify_capabilities.py 门禁；真实语料全路线（本地报告）；KAG 待认证 |

计划 §12 的 P0.2（实例/结果/时钟/版本矩阵与数量上限冻结）作为独立账本待批次 62 交付；本表先指针化，不提前声明。

## 2. 回归账本 R01–R06

计划 §4 P0.1「另设 R01–R06 保留已修复六项回归」——即 NIR-NEXT 计划启动前修复、且 P4 完成门槛要求「均不退化」的六项。每项锚定一个仍在本仓库运行的常设测试；这些测试属于基线，任何后续批次不得删除或放宽。

| ID | 回归 | 修复提交 | 常设测试锚点 |
| --- | --- | --- | --- |
| R01 | 进入剧情后菜单图片资源释放，但对白背景保留 | 8c44ace | `crates/nir-player/src/lib.rs` `story_releases_menu_images_but_keeps_dialogue_background`；tests/browser/release-lifecycle.spec.js |
| R02 | 新激活释放上一激活遗留的活动资源预约 | 8c44ace | `crates/nir-player/src/lib.rs` `next_activation_releases_obsolete_active_reservations` |
| R03 | 媒体准入失败暂停剧情，并以同一激活重试而非丢帧推进 | 8c44ace | `crates/nir-player/src/lib.rs` `admission_failure_pauses_and_retries_same_activation` |
| R04 | 次要/中键指针与未聚焦主键不得触发剧情或标题命中；右键开菜单、历史按钮可见性 | 8c44ace | tests/host/input-routing.test.js；tests/browser/input-routing.spec.js；save-history.spec.js |
| R05 | 预览服务器连接风暴不再阻塞启动 | e9612c2 | scripts/verify_preview_connections.py（CI engine.yml 常设门禁） |
| R06 | Web 设备丢失在宿主 owner 工作前检测，dissolve 中丢失保进度 | 32e36d8 | tests/browser/player.spec.js `an owner turn detects device loss before the idle watchdog`、`actual device loss during a dissolve preserves progress` |

另注：95cbf6e 使仓库可在 Windows 构建与测试（工具链支持），不计入运行时回归；Windows 原生行为仍按上表统一「待验证」。

## 3. 验收样例清单

三个原创中性 fixture（计划 §4 P0.1）；全部为确定性生成内容，不含第三方游戏文本。每个样例走完整作者管线：`novelc resolve → check --locked → test → build --locked`。

| 样例 | 目录 | 覆盖需求 | 验收载体 |
| --- | --- | --- | --- |
| A 夜灯书页 · Reading Lamp | examples/reading-lamp | N01（Await 链）、N04（页内 Gate×2 后继续同一对白、固定 Auto 策略）、N05（wipe 揭示/dissolve 隐藏消息窗口）、N06（场景转场）、N07（BGM gain 0.7、显式语音绑定 sampled_remaining、50 ms 语音淡出停止）、N10（类型化选择结果写入 `kept`） | 场景 sunrise/rest（typed 变量断言）；crates/nir-compiler/tests/p0_examples.rs（能力推导 + VoiceWaitPolicy 断言） |
| B 回想图集 · Replay Atlas | examples/replay-atlas | N06（菜单页进入/关闭转场）、N08（图片菜单组合）、N09（Profile 守卫的锁定回想、Replay 动作以 replay_completed 返回、点击音/页面音乐/元素进入动画）、N10（ui.replay.v1 等能力推导） | 场景 tour；p0_examples.rs（锁定不变量：atlas.north 授予/atlas.south 恒锁；三个入口函数驱动至终态） |
| C 页签设置/存档/历史 | 待批次 62 | N08（stack 菜单、Range/Toggle、存档槽、历史窗口） | player 级验证 + 场景；未实现片段以行为期望描述，不伪装为可编译格式 |

样例 A/B 的 `game.lock` 随仓库提交；`.nir/`、`dist/` 产物按既有 gitignore 规则排除。

## 4. 素材与字体重建

全部素材可从仓库确定性重建，无外部下载：

1. `python3 scripts/make_p0_examples.py [all|reading-lamp|replay-atlas]`——纯标准库重写两棵样例树：PNG（自带 IHDR/IDAT 编码器，渐变+矩形+辉光合成）、WAV（24 kHz 单声道 PCM16 合成音）、TOML/JSON 配置与文本账本（digest 算法与 `nir-format` 的 sha256 紧凑 JSON 一致）。
2. 字体子集不由该脚本生成。母本为仓库 SDK 模板 `templates/minimal/assets/fonts/NotoSansCJKsc-Regular.otf`（Noto Sans CJK SC 2.004，SIL OFL 1.1，静态 CFF1——编译器 `face()` 拒绝可变字体，nix-store 的 CFF2 可变版本不可用）。子集 `assets/source/reader.otf` 由编译器自身的 `fonts::prepare()`（vendored hb-subset，keep_everything + remove_unrecognized_tables + retain_legacy_names + unicode_set）从 `assets/source/reader.chars.txt` 生成：临时在 fonts.rs 挂一个 ignored 测试调用 `prepare()`，完成后撤销；缓存落在样例目录 `.nir/cache/fonts`（gitignore）。
3. 不变量：`reader.chars.txt` = nir-presentation 全部 `.ftl`+`src/**/*.rs` UI 副本闭包 + ASCII 32–127 + 标点 + 样例全部正文 + 游戏标题（标题经 collect_strings 进入每个字体计划，遗漏即 E_FONT_COVERAGE）。新增文字后必须同步扩充字符表并重建子集。
4. 内容变更后重新 `novelc resolve` 更新 `game.lock`。

许可：素材 CC0-1.0（README 声明），字体 OFL-1.1（整份许可随样例提交为 assets/fonts/Noto-OFL.txt，credits/README.md 记录来源与重建指引）。

## 5. 版本/限额矩阵与第二来源（指针）

- **P0.2 版本/上限矩阵**：目标身份、任务结果原因、时间域、页面动作白名单、Replay 策略状态表与数量上限（View 元素/局部数据、表达式深度、计划深度/叶子、可见集合、并发 UI 音频、文本长度）——待批次 62，作为独立账本文档交付；在此之前以 CAPABILITIES.md 与各语义文档为现行记录。
- **KAG 第二来源**：计划 §4 P0.1 要求「至少两个引擎家族……记录精确版本/源码提交/参数/证据方式；无原版运行证据时标待认证」。LiveNovel 侧已有真实语料的映射账本与全路线认证（批次 53/54/59/60，证据保留本地 reports/）；KAG 小型固定子集的版本、来源提交与参数认证待批次 63，在此之前一切 KAG 支持声明均为「待认证」，不写入仓库声明。

## 6. 基线测试记录

- 2026-10-03，批次 61：`cargo test -p nir-compiler`（含 p0_examples 3 项）、`cargo xtask test`（含 verify_capabilities.py 47 项能力一一对应）、`cargo clippy --workspace --all-targets --locked` 通过（19 处现存警告容忍，nir-core 6 处 -D warnings 失败为批次 58 遗留，与本批无关）；两样例 `novelc resolve → check --locked → test → build --locked` 全通过。证据日志：reports/nir-next/batch-61-*.log（本地保留）。
- 本批不触及 Player/格式/编译器运行时代码，按批次 53/55 先例不重跑浏览器整套；R01–R06 的浏览器锚点随最近一次整套运行（批次 58，44 项全绿）成立。
