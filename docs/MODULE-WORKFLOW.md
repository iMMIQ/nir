# 多模块按需加载工作流

本轮目标：三章、两种正文语言、共享资源的作品能按模块执行、按需获取正文，并能在失败、取消和存档恢复中保持正确状态。

## 并行分工

| 工作流 | 所有权 | 交付 |
|---|---|---|
| 编译链接 | compiler project/texts/diagnostics/scenario | 模块命名空间、显式导出、共享声明、跨模块校验 |
| 分包发布 | compiler release | 模块代码与逐语言正文的独立哈希对象 |
| 验收 | 模块测试工程、浏览器测试 | 三章路线、请求闭包、恢复、失败和局部更新 |
| 集成 | format/content/core/player/WebHost | 类型契约、加载屏障、候选提交与失效请求拒绝 |

子代理使用用户指定的 GPT-6 Luna / max effort；公共接口由主代理统一维护。先冻结接口，再独立实现，最后运行集成回归。

## 本轮格式边界

引导执行对象保存模块接口、共享变量和静态声明目录；函数体按模块分包，正文按模块和语言分包。静态目录仍包含场景、Cue、选项、文本契约和资源元数据，因此本阶段不宣称整个 IR 已达到最小章节下载闭包。字体继续使用现有逐语言计划，尚未拆成章节字体。

内容对象不包含全作品修订，避免一章译文变化使无关模块对象失效。会话与存档仍固定精确发行身份，不提供跨发行迁移。

## 作者接口

在 `game.toml` 中显式登记共享声明和模块：

```toml
[inputs]
shared = ["content/common/definitions.nir.json"]
modules = ["content/ch01/module.toml", "content/ch02/module.toml", "content/ch03/module.toml"]
# 保留既有 asset_catalogs、theme、player、locales 等配置。
```

共享 fragment 当前只接受 `variables`，不接受函数、场景、Cue 或选项。多模块工程的全局变量在这里定义，模块加载不再初始化这些值。旧单模块工程仍可在其 fragment 中定义变量。

每个 `module.toml` 保留现有 sources、text_contracts、text_revisions 和 text_bundles，并声明导出：

```toml
module_format = 1
id = "ch02"
sources = ["story.nir.json"]
text_contracts = "texts/contracts.json"
text_revisions = "texts/revisions.json"

[text_bundles]
zh-Hans = "texts/zh-Hans.json"
en = "texts/en.json"

[exports]
start = "main"
```

多模块工程的跨模块 Call 使用 `ch02.start`，链接到该模块导出的实际函数。未导出的私有函数不能跨模块访问。模块内使用本地名字；函数、场景、Cue、Choice、文本和任务引用由编译器限定模块身份。Choice 内部的 OptionId 保留原 ID，场景节点符号仍遵循已有场景/代次语义。第一模块的 `start` 导出是新游戏入口，模块数组不是自动播放顺序。

旧单模块工程保留裸逻辑 ID；添加第二模块会启用模块命名空间，属于内容身份变化，需要重建且不支持旧发行存档迁移。模块内移动源文件不会改变逻辑 ID。

多模块翻译维护使用 `module.text` 选择一条文本：

```sh
novelc -p my-story text status
novelc -p my-story text update --id ch02.intro --meaning preserve
novelc -p my-story text review --id ch02.intro --locale en
novelc -p my-story check --locked
novelc -p my-story build --locked
```

## 运行与失败政策

- 标题阶段只载入静态根、运行时与标题所需媒体。新游戏首次执行才请求入口模块函数体及所选正文。
- 缺少被调用函数或即将实例化的正文时，核心在原 PC 停止；不执行 Call、不重新求随机数、不分配新交互实例，也不推进剩余 Story 时间。
- 模块代码和正文先在隔离候选中校验哈希、身份、接口与文本契约，再整体安装。准备失败保留旧场景并提供重试或返回标题。
- 正文切换先获取当前模块的候选语言，再使用原有字体/布局准备协议提交。已出现的对白、选项和历史继续使用冻结的内容。
- 读档根据调用栈及冻结对白找出必要对象，先准备并验证候选。栈中仍引用的旧章函数可能属于恢复闭包，但不会重放旧章入口。
- 取消以请求身份拒绝迟到结果；网络中止只是节省资源。设备变化不使经过校验的模块字节失效。

当前以固定大小包完整校验后解析，不是字节流解释。每批最多 128 个对象、16 MiB 内容字节，合并后的序列化 Program 上限仍为 16 MiB；超过限制明确拒绝。已加载代码和正文在播放器生命周期内保留，尚未实现模块淘汰。资源账本和这些限额不代表物理内存测量。

## 集成关卡

1. 原有单模块工程与编译器回归保持可用。
2. 跨模块只调用导出；共享变量不重复初始化；未声明引用在构建期拒绝。
3. 新游戏和跨模块执行在显式边界请求内容；失败保留当前场景，过期完成不可提交。
4. 存档先准备所需模块，再验证候选并准备媒体；不重放新游戏入口。
5. 浏览器验证实际请求对象、语言实例冻结、重复/迟到请求及恢复。
6. 记录实际执行的测试和未验证环境，再更新能力说明。

## 验收标准

- 阅读第一章不下载后两章函数体和正文，也不下载未选正文语言。
- 第二章存档恢复不会再次执行第一章的共享变量初始化。
- 模块获取失败可重试；返回标题后旧请求不能推进剧情。
- 只改第二章英文时，其他章节正文和无关媒体对象摘要不变。
- 现有单模块语义、语言、字体、存档与设备恢复回归通过。

## 验证记录（2026-09-23）

在 NixOS 开发环境运行：

- `cargo xtask test`：109 项通过，实际依赖图检查通过；最终播放器修改后再次执行 `cargo test --locked -p nir-player`，40 项通过（含新增的下载完成后准备失败诊断测试）。
- `cargo test --locked -p nir-presentation`：3 项通过。
- 原生 workspace 与 `player-web` WASM 的 `cargo clippy ... -- -D warnings`、`cargo fmt --all --check`、`git diff --check` 通过。vendor/wgpu 的两项既有编译警告仍存在。
- `npm run test:host`：22 项通过。
- 已有浏览器用例共 25 项在分组执行中通过；主题用例曾发生一次准备超时，独立复测通过。Chromium/X11 的分数设备缩放会令 390px 请求视口变成 391px，焦点框断言已改为验证实际左右 20px 边距。
- `npm run test:browser -- tests/browser/modules.spec.js`：新增 3 项通过，覆盖首章/语言请求闭包、共享背景跨章复用、第二章读档且不重放入口、获取失败重试、取消后迟到结果不推进剧情，以及仅改第二章英文的对象摘要隔离。连同已有用例共 28 项；按分组执行，非单次全量执行。
- `cargo xtask sdk`、`verify_release.py`、`verify_sdk.py` 通过，包含独立 CLI、无 Cargo 作者环境、重复构建和逐语言分包验证。

浏览器环境为桌面 Chromium 153 + Xvfb + SwiftShader WebGPU；本记录不代表移动设备或实体 GPU 验证。后续里程碑包括静态目录进一步分包、章节字体、模块淘汰与内存测量，以及作者 Profile/Edition 流程。
