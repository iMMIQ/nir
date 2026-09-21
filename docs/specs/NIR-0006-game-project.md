# NIR-0006：作品工程、内容组织与用户工作流

状态：设计提案 v0.1。日期：2026-09-20。

范围：制作或维护一部 Galgame 的用户侧内容工程。承接 NIR-0001 至 NIR-0005，播放器保持 Rust/WASM＋wgpu，首版只发行 Web；不重复设计引擎仓库，不设计旧格式导入，不要求作者编辑器，不引入新剧情语言。

本文件中的 `game.toml`、fragment 载体和 `novelc` 命令是拟议接口。附带作品是结构样例，没有可执行 SDK、图片、BGM、配音或字体。Python 检查器只验证部分工程结构，不是 NIR 编译器或播放器。

## 1. 产品单位：一部作品是内容项目，不是一个引擎分支

默认用户流程：获得受版本管理的 SDK/CLI → 创建工程 → 放入符合契约的 NIR 与资源 → 修改配置/主题 → 浏览器预览 → 检查 → 构建静态站点目录。

普通作品工程不包含 Cargo.toml、wgpu 依赖、Rust target、wasm-bindgen glue、GPU shader 缓存或一份复制的引擎源码。普通内容与主题改变不触发用户手动重编引擎。确有引擎代码改动时，维护独立 SDK 构建并通过锁文件引用；不能把引擎补丁藏在每部作品里。

源工程、构建工作区、发行包、玩家数据为四个不同的产品对象：

| 对象 | 谁维护 | 是否版本控制 | 是否默认公开 |
|---|---|---|---|
| 源工程 | 用户及规范化工具 | 是 | 否 |
| `.nir/` 工作区 | 构建工具 | 否 | 否 |
| `dist/` 发行包 | 构建工具 | 用发行对象库归档 | 是 |
| 浏览器存档/偏好 | 播放器与玩家 | 不放进源仓库 | 否 |

## 2. 一份最小工程与一份可扩展工程

最小版可以只有 game.toml、一个 module.toml、一个 NIR fragment、一个文本包、资源 catalog 和素材目录。其他功能由已锁定 SDK 的默认方案提供。不要求用户为了显示第一句对白先填写几十个文件。

可扩展形态：

```text
rain-letters/
  game.toml
  game.lock                    工具生成；模板仅提供 .example 文件
  game.local.toml               可选、忽略版本控制
  content/
    common/definitions.nir.json
    ch01/
      module.toml
      story.nir.json
      scenes.nir.json
      texts/contracts.json
      texts/zh-Hans.json
      texts/en.json
      voices/ja.toml
    ch02/                      以后按同样规则添加
  assets/
    catalog.toml               可扩展为多个显式注册的 catalog
    source/                    准许进入处理流水线的原始素材
  themes/rain/
    theme.toml
    tokens.json
    components/                必要时才增加
  config/
    player.toml
    locales.toml
    web.toml
  editions/full.toml
  profiles/dev.toml
  profiles/release.toml
  tests/scenarios/
  credits/assets.toml
  schemas/
  .vscode/settings.json
  .nir/                        可删、可重建
  dist/                        构建生成
  reports/                     构建/校验报告
```

目录是默认约定，真正纳入工程的文件由清单显式引用。不能因为文件放在目录中就自动发布，也不能依文件名字母顺序决定剧情执行顺序。

章节中的演出与文本聚合在一起，便于代码评审、章节语言覆盖检查和分包。公共素材由目录按 ID 复用，不在每章复制一份。

## 3. 根清单与配置责任

`game.toml` 只回答：这是哪部作品、依赖哪类 SDK、舞台与基础语义是什么、入口和配置文件在哪里。

```toml
project_format = 1

[game]
id = "org.example.rain-letters"
slug = "rain-letters"
title = "雨后书简"
version = "0.1.0"
source_locale = "zh-Hans"

[engine]
api = "nir-player/0.1"
capability_profile = "web-v1"
runtime_preset = "web-standard"

[stage]
width = 1280
height = 720
origin = "top_left"
y_axis = "down"
fit = "contain"
default_compositing = "linear_premultiplied"

[inputs]
shared = ["content/common/definitions.nir.json"]
modules = ["content/ch01/module.toml"]
asset_catalogs = ["assets/catalog.toml"]
theme = "themes/rain/theme.toml"
locales = "config/locales.toml"
player = "config/player.toml"
web = "config/web.toml"
credits = "credits/assets.toml"
editions = ["editions/full.toml"]
profiles = ["profiles/dev.toml", "profiles/release.toml"]
scenarios = ["tests/scenarios/walk.toml", "tests/scenarios/stay.toml"]
```

固定使用 TOML 1.0 子集可以提供注释与明确类型，但不是宣称它是当前最新 TOML 版本。[R1] NIR 和复杂结构继续用 JSON，不强行把所有数据转成同一语法。

职责分配：

- 项目语义：GameId、NIR 版本、stage、剧情入口。不能被本机设置悄悄覆盖。
- 内容呈现：阅读默认值、对话框布局、主题、语言支持。
- 构建政策：资源处理档位、调试信息、构建验证、目标 Web。
- 部署参数：base path 等公开配置，不能放密钥。
- 本地工作设置：端口、日志、开发测试选择。Release 默认拒绝语义相关本地覆盖。
- 玩家偏好：字号、音量、减少动态等，已有偏好优先于作品默认；不会解除 hard gate 或删除必需任务。

不提供一个可任意深度合并所有文件的配置魔法。每个字段有归属和允许覆盖范围。未知核心字段报错；工具可输出最终配置及每个字段的来源。

`game.toml` 不是必须加载进浏览器的配置。`novelc` 应把必要字段编入已解析运行清单，剥离本地工作信息。

## 4. 锁文件、缓存与可复现性

`game.toml` 声明所需契约和能力；`game.lock` 固定具体 compiler、runtime WASM/JS/Worker、默认主题、语言数据、资源处理器及各自身份。使用这个模式是借鉴清单和精确解析结果分离的做法，不要求用户工程本身使用 Cargo。[R2]

`game.lock` 工具生成并提交版本控制；不可用一串伪造 hash 充当已经解析的 SDK。本样例故意只有 `game.lock.example.toml`。

`novelc check/build --locked` 在锁不存在、与输入要求不一致、解析需要漂移时失败。显式 `resolve/update` 才改变锁，并报告兼容性影响。SDK 本机目录覆盖只用于开发；正式发行要求可固定、可验证的构建身份。

源码内容 hash 由构建图自动计算，不要求用户每改一句对白就更新手写清单。`.nir/` 可以包含解析索引、增量缓存和处理中间产物，但不能保存唯一的 AssetId、TextId 或项目身份。删除缓存不丢失任何权威信息。

锁文件不是可重复构建的全部：仍需固定资源编码/语言数据、排序和输出政策，并独立重建比较。

## 5. 稳定身份、物理路径与内容 hash

| 身份 | 变化时机 | 用途 |
|---|---|---|
| GameId | 新建独立作品/明确 fork | 存档、更新、内容命名空间 |
| ModuleId、ExportId、OpId、TextId | 语义身份真正改变时 | 引用、恢复与测试 |
| AssetId | 换成一个逻辑上不同的资源时 | 剧情与主题引用 |
| source path | 移动或重命名文件时 | 源文件定位 |
| object hash | 最终字节改变时 | 缓存、校验、发行身份 |

显示标题、项目文件夹和 URL slug 改名，不应改变 GameId。复制一部工程制作真正的新作品，则显式生成新 GameId，防止共享存档命名空间。

逻辑 AssetId 不由文件路径每次重新计算；初次注册时可以从文件名生成建议键，但一旦写入目录即持久化。移动文件只更新 source。Godot ResourceUID 也体现了“资源引用不因移动/改名断开”的设计目标；本方案不复制其 UID 格式。[R3]

模块引用使用结构化 ModuleId/ExportId；样例简写 `ch01.start`。v0.1 的 ModuleId 和 ExportId 简写不含点，避免字符串分割歧义。资源与其他局部符号可以使用点状可读名字，类型不同的身份不混用。

重命名逻辑 ID 是 API/内容迁移，不是普通文件移动。显式别名或迁移表仍需要验证；稳定 ID 不自动证明存档跨内容版本兼容。

## 6. NIR 如何放进作品工程

### 6.1 不再引入第二门剧情语言

第一版直接使用已经设计好的 NIR 语义。用户已有的合规 NIR 可进入模块；不把这些工程文件描述成旧格式转换器。

源工程支持将 Program 的表拆成 fragment 文件。例如：

- shared definitions：变量声明、说话人、UI 契约、阅读策略。
- story：functions、choices、cues、相关 hints。
- scenes：scene_templates、clips。
- 文本：独立 TextContract 与各语言的结构化 TextDoc。

`fragment_format = 1` 是源文件封装，不是新的运行 opcode。它不是一份可以独立直接交给旧 NIR loader 的完整 Program。项目装配器会解析、链接并生成标准 Program/Executable。

### 6.2 装配规则

每条记录只能有一个权威定义。多个文件可以贡献同一张表的不同记录；同类型同身份重复即报错，禁止“后读到的文件覆盖前一个”。显式主题继承在主题规则内处理，不推广为全局数据覆盖。

模块本地函数/场景符号由 ModuleId 限定，共享声明一次定义、多个模块引用。共享变量初值只在新会话初始化时应用，不因下载下一章再次初始化。检查器为了静态引用验证复制 Python 字典，不代表运行时复制状态。

源文件顺序不能改变控制流。数组中本来有语义的顺序（图层、选项、spans、事件）必须保留；不能对所有 JSON 数组排序来实现稳定输出。

### 6.3 模块契约

```toml
module_format = 1
id = "ch01"
sources = ["story.nir.json", "scenes.nir.json"]
text_contracts = "texts/contracts.json"
text_bundles = ["texts/zh-Hans.json", "texts/en.json"]
voice_catalogs = ["voices/ja.toml"]

[exports]
start = "main"
```

入口由 edition 选择，例为 `ch01.start`。跨模块只引用具名导出和经过验证的接口，不能随意跳进另一个文件的内部块。模块运行时尚未加载时由既有显式准备边界处理，不恢复字符串脚本解释。

源文件包含图、主题继承图的非法循环应报错；故事中的循环、跳转和返回按 NIR 语义处理，不能误当成文件依赖错误。

首版不必提供任意 fragment include、运行时宏和自定义导入器生态。样例保持一个模块，可以精确说明结构。

### 6.4 作品启动、读取和回放

播放器初始化与作品新游戏初始化分开。标题、设置、读档、错误恢复由默认播放器提供，作品可以替换对应 UI；Edition 的入口只用于开始新会话。读取存档不重新执行新游戏入口或再次应用变量初值。

角色定义（SpeakerId）和可见立绘节点分开；背景/角色站位属于 SceneTemplate；并行动效属于 Cue/Clip；章节选择属于具名入口和允许访问规则。它们应按已有 NIR 表表达，不在项目根另外维护一套会与 NIR 冲突的剧情状态。

鉴赏和场景回放是显式功能根。如果支持重播，需要隔离的播放会话和明确的 Profile 写入政策，不能修改玩家当前主线存档。第一版可以不实现鉴赏，但不能在实现后漏算其资源依赖。

## 7. 资源目录与处理策略

```toml
format = 1

[[assets]]
id = "res.actor.aki"
kind = "image"
source = "source/actors/aki.webp"
role = "character"
color_space = "srgb"
alpha = "straight"
expected_size = [600, 1000]
pipeline = "character"
rights = "project-media"
```

文件路径相对于包含它的 manifest 所在目录，所以这里实际定位 `assets/source/actors/aki.webp`。工程根清单中的路径则相对于工程根。所有规则明确，不能同一字段有时根相对、有时 cwd 相对。

第一版只接受项目内相对路径，不隐式执行环境变量/任意 URL，不允许 `..` 和绝对路径逃逸。外置大型资源可通过固定、可校验的显式挂载在后续支持，不作为不受约束的 symlink。

`expected_size` 是作者断言，不是自动侦测结果。工具探测真正尺寸、透明度与音轨信息，断言不符时报差异；音频循环点需由实际内容确定。

`role/pipeline` 表达用途和允许的处理策略。用户不维护显存纹理格式、上传批次、hash URL 或手工 mip 索引。背景、透明立绘、遮罩、字体和音频采用合适的默认处理方案，并允许确有需要的资源级参数。

用户已有 res/ 目录可以原样保留，只需把 catalog 放在合适位置并显式引用相对路径；assets/source/ 是默认约定，不是强制搬迁。资源登记与旧脚本转换是不同工作，本文不处理后者。

目录可以按角色/章节拆分 catalog，避免单个文件过大，但每个 AssetId 只在一个 catalog 声明。已知资源集合从 catalog 建立；没有登记的文件不自动变成发布资源。使用图从 NIR、主题、画廊/标题及显式 runtime roots 建立，不只扫描正文。

任意字符串拼接出的资源名不得绕过依赖闭包。动态引用必须有受限域或明确候选集合，构建才能验证并打包。

工作母版、未使用版本和备份即使位于源仓库，也不默认进入发行包。只复制整个 assets/ 是不允许的发布策略。

## 8. 文本、角色、配音与语言的编辑责任

同一章的语言包放在该章目录。它们共享 TextId 和契约，不复制整个剧情。

- 控制流只引用 TextId、SpeakerId、OptionId、VoiceKey/AssetId。
- 说话人声明指向显示名 TextId，不强制对应某个立绘节点。
- 语言包包含结构化 spans、Ruby 和 Gate 位置。
- 配音目录用逻辑键明确绑定，不依赖文件排序。
- 菜单消息跟随 SDK 的 Fluent 消息体系；章节正文不转换成可执行的 FTL 字符串。

源文件用整数 `source_revision/contract_revision` 表达翻译针对哪版契约；构建器自动生成和验证 `contract_digest`，不要让作者手工抄 hash。

更改 Gate、选项含义和参数类型要显式变更契约，并标记翻译待复核。普通文本改错也更新源修订；MeaningRevision 是否变化需要内容决策，而不是简单等于每次字节 hash。

配音与正文语言独立。正式声明支持某语言章节时必须完整；未完成语言不能随机逐句混入默认语言。配置希望支持某字体或音轨，不证明对应缺失文件已经存在。

样例提供完整的简中/英文文字结构和日文配音绑定声明，但没有实际配音或字体，因此不能作为语言播放验收结果。

## 9. 主题是作品的一部分，但不是另一套播放器

```toml
format = 1
id = "theme.rain"
base = "builtin.reader"
tokens = "tokens.json"

[slots]
"dialogue.main" = "builtin.dialogue"
"choice.main" = "builtin.choice"
```

推荐用户首先改 token，然后替换受契约限制的组件。必要时再增加页面组合，不从第一天重写存档事务、输入令牌与阅读状态机。

`builtin.*` 的实现由锁定 SDK 提供，样例中的 ID 只是拟定协议。组件 props、允许的 action、焦点与辅助语义契约必须验证。

主题不能调用任意 Rust/JS、直接改剧情变量、关闭所有退出入口或覆盖玩家无障碍偏好。高对比、字号与减少动态仍由播放器保留最终控制。

标题页、CG 鉴赏等引用的素材也参与 runtime dependency roots；不能为了只按剧情扫描依赖而漏打标题图片，也不能让试玩版的鉴赏页保留全作隐藏 CG。

## 10. Edition、Profile 与 Deployment 是三个轴

- Edition：完整/试玩等内容边界，决定合法入口、模块范围、语言/配音组合与显式额外根。
- Profile：开发/发布的诊断、资源质量、验证政策，不改变剧情分支与 save contract。
- Deployment：同一产物的公开 base path/托管策略，不放秘密。

```toml
format = 1
id = "full"
entry = "ch01.start"
modules = ["ch01"]
text_locales = ["zh-Hans", "en"]
voice_locales = ["ja"]
extra_runtime_roots = ["res.bg.station"]
```

`modules` 是许可集合，不是首次加载清单。启动依赖另由准备计划决定。

试玩不能用一个 runtime bool 把内容藏起来，却把完整版脚本/CG 发到浏览器。应有明确的试玩入口/终点和闭合依赖。引用排除模块或未许可的 full-only 资源时报错，不静默删掉分支。

同一 GameId 下可以设计 save_lineage 区分试玩/完整。两者互通必须显式支持并测试，不从名字自动猜测。只有真正独立作品才生成新 GameId。

禁止把 Edition 覆盖扩展为一套任意 patch 语言。第一版用明确模块和入口即可。若资源质量处理会改变必要演出，它就不是普通 Profile 开关。

## 11. 用户命令与日常变更

以下全部是待实现的 CLI 设计：

```sh
novelc init rain-letters --template web-basic
novelc resolve
novelc doctor
novelc check --locked
novelc dev --scenario tests/scenarios/walk.toml
novelc build --target web --edition full --profile release --locked
```

`init` 生成独立 GameId、最小内容和默认配置；`resolve` 固定 SDK，首版不要求有通用网络包注册中心。`doctor` 解释缺少的工具/媒体/LFS/字体/协议。`check` 验证图而不重做全部媒体。`dev` 使用正式 Rust/wgpu 播放器的开发模式，提供受限制诊断与可复现入口；不是另一份 JavaScript 剧情实现。`build` 从锁定图生成静态发布目录和报告。

推荐修改反馈：

| 用户修改 | 主要失效范围 | 正在播放的会话 |
|---|---|---|
| 一句译文 | 本章该语言文本/相关字体数据 | 新实例或安全边界应用 |
| UI 颜色 | 主题数据 | 准备成功后更新 |
| 布局/字号默认 | 相应呈现与布局 | 保持当前剧情，重排 |
| 立绘源图 | 该资源派生物 | 保留旧资源至安全提交 |
| 分支/任务结构 | 对应 NIR 模块与恢复表 | 检查点重启或受验证迁移 |
| SDK 版本 | 运行时及契约验证 | 明确重启，不热换 WASM |

编译错误保持上次有效预览并标记过期，不能把红色错误覆盖掉后假装正在看最新内容。变更从缓存完成到激活继续遵守 ReadyLease 与语言安全边界。

开发热更新不等于发行页静默读取最新资源；Release 会话继续锁定精确对象图。

## 12. 用户自己的测试工程

`tests/scenarios` 是剧情测试配方，不是截取的玩家存档。首版推荐从 new_game 或合法具名前置状态开始，按逻辑交互驱动：

```toml
format = 1
id = "walk"
mode = "new_game"
entry = "ch01.start"
text_locale = "zh-Hans"
voice_locale = "ja"

[[steps]]
action = "await_choice"
id = "choice.route"

[[steps]]
action = "choose"
option_id = "walk"

[expect]
outcome = "walk_home"
affection = 1
```

此处为片段；随包完整样例在选项前还等待并推进对白。逻辑测试使用 ID，不靠像素坐标和固定 sleep。触摸/布局端到端测试单独做。

从章中段预览必须给定可验证的检查点、前置状态或从入口重放，不能只设置 PC 后假装先前背景、局部变量和任务存在。

UI 默认、主题、语言与资源质量都可以成为测试矩阵。剧情回归断言关注 outcome、变量、Gate 和恢复，不仅是截图相似。

开发存档命名空间应包含稳定 GameId 和开发实例/用例隔离标识，不共用正式槽位。不建议每次编译把 namespace 改成全部产物 hash，以免开发中无法测试升级恢复。

浏览器数据由 origin 管理，不是源项目某个 saves/ 文件夹。[R6] 改变本机预览端口可能改变 origin；固定端口、显式导入导出和开发隔离更可控。

## 13. 校验、诊断与编辑支持

不开发作者 GUI 也可以提供良好的编辑体验：SDK 生成本地 JSON Schema、给出补全和 hover，再由 novelc 承担跨文件与语义校验。VS Code 能按文件模式关联本地 schema；这不自动验证动态的项目符号图。[R4]

样例 schema 仅检查 fragment/text envelope，明确不是完整 NIR schema，避免把编辑器不报红误当成程序正确。

项目诊断应包含稳定错误码、源文件位置、引用链和可执行修复提示，例如：

```text
E_ASSET_UNDECLARED
content/ch01/scenes.nir.json: scene.station / aki.asset
  references: res.actor.aki_sad
  declared catalog: assets/catalog.toml
  action: register a resource or correct this logical reference
```

正式 `check/build` 至少应覆盖：重复身份、非法路径、大小写/Unicode 规范化碰撞、丢失源文件或 LFS pointer、错误资源类型、翻译契约、组件动作、跨模块导出、试玩依赖泄漏、锁文件与能力不匹配、恢复映射、安全限额和未确定的来源记录。

合法内容循环交给 NIR 语义验证，不用目录或字符串排序代替。

## 14. 版本控制与发布目录

提交：game.toml、game.lock、内容与映射、主题、测试、许可记录、必要源素材（或可恢复的受控素材对象）。忽略：.nir、dist、reports、本机设置、玩家存档、凭据。

大二进制可以选择 Git LFS，但 Git 中保存的是 pointer，实际文件另存；CI 必须拿到内容字节，构建检查器应拒绝把 pointer 当图片。[R5] 没配置好存储与 CI 前，不应给模板默认启用一堆 LFS filter 让用户克隆后无法运行。

输出大致为：

```text
dist/full/web/
  index.html
  bootstrap.js
  channels/stable.json
  releases/<digest>.json
  objects/<digest>.<ext>
```

这是用户上传的运行目录，不是整个源码仓库。不包含未使用母版、工作脚本、测试状态或 deployment secrets。构建产生文件清单、包体分项、依赖链、版本/来源清单和潜在兼容性变化报告。

`game.toml`/lock 是输入，ReleaseManifest 是输出；不维护两份手写资源图。发布原子入口、缓存和旧会话兼容沿用 NIR-0003。

## 15. 范围与实施次序

P0：根清单、模块装配、逻辑资源、默认主题、语言文件、锁、check/dev/build、规范路径、一个完整示例和两条剧情用例。

P1：多模块的引用导航、资源注册/改名辅助、更多编辑器诊断、试玩发行检查、热变更差异报告。

P2：面向人工书写的高层语法、可视化编辑器、团队在线协作、公共内容包市场。将来这些工具都应写回同一源数据或明确的一向生成链，不产生两份可同时修改的权威剧情。

不要先制造一个通用包管理器、任务 DSL 或无限 include/patch 配置系统。第一部作品应能用最少的配置运行，同时在规模增大时保持模块与身份稳定。

## 16. 附带样例与验证边界

`rain-letters/` 是结构样例。故事表保留前序 example.nir.json 的基本内容，拆分为公共定义、故事、场景、文本和资源目录。没有旧引擎转换。

`check_project.py`：Python 3.11+ 标准库，实际可执行。结构模式允许缺媒体/SDK但报告 notices；release 模式阻断这些缺失。它不证明真实 SDK 可用，不验证音频格式、字体塑形、完整 NIR 类型系统、回退/任务语义、最终发布闭包或 GPU。

实际验证记录见 test-results.txt：20 项结构检查器测试。两个 scenario 仅做引用验证，未在真实播放器执行。严格发布检查有预期错误，见 release-blockers.json，不能将结构模式 passed 描述为“项目已可发行”。

本包不含字体文件，也不提供虚构的运行时下载、digest 或锁文件。实际 SDK 和合规媒体补齐后仍需完整 novelc 与浏览器测试。

## 参考资料

以下来源用于验证通用工具事实；本文工程文件格式和命令是自定义提案，不是这些项目提供的现成功能。

- [R1] TOML 1.0 规范（注释、类型、UTF-8）：https://toml.io/en/v1.0.0
- [R2] Cargo manifest/lock 分离：https://doc.rust-lang.org/cargo/guide/cargo-toml-vs-cargo-lock.html
- [R3] Godot ResourceUID：https://docs.godotengine.org/en/stable/classes/class_resourceuid.html
- [R4] VS Code JSON Schema 编辑支持：https://code.visualstudio.com/docs/languages/json
- [R5] GitHub Git LFS pointer：https://docs.github.com/en/repositories/working-with-files/managing-large-files/about-git-large-file-storage
- [R6] IndexedDB 基础术语与同源范围：https://developer.mozilla.org/en-US/docs/Web/API/IndexedDB_API/Basic_Terminology

前序规范：NIR-0001（语义）、NIR-0002（准备/租约）、NIR-0003（语言/发布）、NIR-0004（编译/恢复映射）、NIR-0005（引擎模块边界）。
