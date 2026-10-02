# 外部引擎导入（实验性）

导入工具由 Rust 实现，直接编译进 SDK 配套的 `novelc`。不需要 Python、原引擎运行时或额外转换器；不会启动游戏 EXE 或加载 DLL。播放器不依赖导入器。

目前提供 **LiveMaker LSB 116** 适配器，输入为已经解包的游戏目录。它是受限的迁移工具，不是完整的 LiveMaker 模拟器。EXE/DAT 归档解包、其他 LSB 版本暂未支持。

## 检查原包

```sh
./novelc import inspect "/path/to/extracted-game" > inventory.json
```

无需 SDK 文件即可检查。JSON 包含文件扩展名数量、各脚本的 SHA-256、版本、指令数量、TpWord 字形类型和系统事件名；不输出剧情正文或事件参数。解析失败时保留其他脚本的结果，并以非零状态退出。字形类型为原格式的十六进制编号。

## 生成 NIR 工程

```sh
./novelc import livemaker "/path/to/extracted-game" --out imported-story \
  --game-id org.example.story --title "My Story"
./novelc -p imported-story resolve
./novelc -p imported-story check --locked
./novelc -p imported-story build --locked
```

默认从 `live.lpb` 的启动脚本进入；也可指定 `--entry subdir/story.lsb --line 90`。路径以游戏根目录为基准，兼容 Windows 反斜线及 `.lsc` 引用。`--line` 是原脚本 Label 的 `LineNo`，不是数组下标；0 表示文件开头。跳转目标缺失或歧义会报错。

`--out` 必须不存在，父目录必须已存在，且不能与源目录重叠。生成过程使用临时目录，经过正文修订、字体和 NIR 编译验证后才交付工程。源目录不修改。通用 LSB 路径只导入正文；识别到下述 LiveNovel 配置时，会把引用到的原图、声音转换到工程内，并生成 `import-media.json`，记录来源、SHA-256 和转换文件。素材版权仍属于原权利人。

正文语言默认 `ja`，也接受 `--locale en` 或 `--locale zh-Hans`。这是语言标识，不会翻译正文。界面使用英文，可通过作品配置改为中文。模板的 Noto CJK SC 字体覆盖日文常用字形，但采用 SC 字形设计；需要日文字形风格时应换成合适的 JP 字体母版。

## LiveNovel 事件、菜单与媒体配置

提供受限的 **LiveNovel 116 线性／选择剧情与回想配置**。从 `ノベルシステム/START.lsb` 启动、带 `menu.lpm` 和回想分发脚本时自动识别。适配器读取实际主线跳转、选择分发、回想目标、解锁 ID 与 LPM 按钮；替换标准系统脚本。当前配置的舞台、消息框和系统资源约定为 1024×768，不代表所有 LiveNovel 作品都能直接导入。未知事件或不匹配的剧情结构会报错，不会静默忽略。

- `CREATECG`、`CHANGECG`、`DELETECG`：图层、位置、叠放顺序、纯色画面；替换不存在的图层时按原约定创建。原 wipe 用相同时长的 dissolve 替代。
- `PLAYSND`、`STOPSND`：BGM 循环、语音替换、翻页停止非循环语音、原音量；WAV PCM16 与 Ogg/Vorbis 转成播放器支持的 WAV，六声道 WAV 下混成立体声。停止淡出保留时长，非循环语音翻页时以 50 ms 淡出；事件音量写入 Audio.gain，在播放时与玩家总线音量相乘，PCM16 不再预乘事件音量，系统音量初值从 LPB116 的作者默认配置换算为 NIR 值。
- `WAIT`、`MESON`、`MESOFF`、消息结束握手：顺序等待和消息框显隐。页内事件使用 NIR Gate 保留在文字中的执行位置；消息框淡入淡出目前以立即显隐替代。
- 标题菜单：从原背景、按钮普通／悬停图片和 LPM 坐标自动生成有限 MenuElement，等比例适配视口，指针点击与 Web 键盘／辅助语义。
- 回想：原缩略图与网格坐标、独立入口、Profile 解锁，读完返回回想菜单。锁定项使用保留透明度的黑色替代图并拒绝执行；另保留 NIR 系统菜单和返回按钮，便于触摸访问。
- 自动阅读：从 LPB 的 StatusAutoTextWait 读取固定等待，语音余量在 Auto 周期开始时采样并冻结，不再使用文本长度附加或并行语音等待。零等待有显式能力声明。
- 剧情选择：识别标准選択メニュー约定——无条件调用 `ノベルシステム\選択メニュー\■選択実行.lsb`（调用参数与菜单外壳由 NIR 替代），其后紧跟对 `選択値` 变量与单个字符串字面量做相等比较（操作 12）的连续条件跳转分发链，链尾必须是 Exit（字面量恒命中，后继不可达）。每个分发字面量即选项文本，也是回调提交进 `選択値` 的值（選択.lsb 证据：`選択値 = @ParamStr[0]`）。整个调用点降级为一个类型化 Interact：选项声明字符串值、VM 独占写入 `選択値`、分支目标继续各自标签处的路线；合流分支合并为同一续块，回到自身可达路径的分支按路线循环拒绝。条件调用、缺失/单选项链、重复选项文本、链后非 Exit 一律 `E_IMPORT_CHOICE` 报错。菜单皮肤、悬停/选择音效、倒计时与对齐参数不迁移，报告保持 `converted_with_adaptations`。
- 对白：原消息框图片、位置、透明度、白色文字、32 像素基础字号和 40 像素行高。字体采用随 SDK 提供的字体；揭示间隔映射 live.lpb 的 `StatusTextSpeed`——单位为每字符毫秒（由固定滑条回调 `StatusTextSpeed = @ParamStr[0] × 64` 与官方文档的每字符毫秒语义双重认证，0 表示瞬时），换算为每字素簇微秒。

GAL 105/106 支持有界的单帧 8/24/32 位图、原始／zlib 数据、块引用、透明度、调色板、图层合成与尾部矩形列表。动画 GAL、LCM 视频和归档解包尚未支持；本配置不会导入未引用的动画光标。解码由 Rust 在同一个 `novelc` 内完成，没有外部媒体进程。

**尚未等同于原引擎的部分：** 存读档、设置、历史记录使用 NIR 系统界面；旧 LiveMaker 存档不兼容。标题／回想选择音与回想 BGM、消息框渐变和 wipe 以有界 NIR 机制映射（账本记为 adapted）；悬停音效、动画光标和逐字符原字体样式尚未复刻。报告逐项列出这些差异；不是无差异转换认证。

### 映射级别、证据与近似接受（报告格式 2）

`import-report.json` 的 `mappings` 数组按规则记录兼容结论，行为级别与证据强度分开：

- **级别（行为结论）**：`exact`（行为经等价机制保留）、`adapted`（以不同但有界面的 NIR 机制替代，差异已记录）、`approximate`（存在已知分歧、等价性未认证）、`unsupported`（无法映射；严格模式报错，草稿降级为故障块）。
- **证据（结论依据）**：`documented`（公开格式文档）或 `decoded-source`（从固定源码解码定型）。原版实机对照与跨后端验证尚未作为证据类出现，相关未决项保留在近似说明与保真警告中。
- 每条记录包含规则 ID、源版本（LSB116／LPB116／LPM106／GAL105/106）、规范行为一句话、依赖的目标能力与近似位置/未决项；公共记录不含私有路径或正文。

聚合状态由账本推导：含 `approximate` 时为 `converted_with_approximations`，否则含 `adapted` 为 `converted_with_adaptations`，全部 `exact` 才是 `converted`。近似不是可忽略的 warning：未显式接受的近似规则会让命令在工程与报告写出后以 `E_IMPORT_APPROXIMATE` 退出，并逐个点名规则；接受必须按规则 ID 显式给出：

```sh
./novelc import livemaker "/path/to/extracted-game" --out imported-story \
  --game-id org.example.story --title "My Story" \
  --accept-approximate livenovel.menu-hover,livenovel.text.font
```

拼错的 ID 不会静默通过——真实规则仍未接受并被点名。当前 LiveNovel 配置的近似规则固定为 `livenovel.menu-hover`（悬停音效／动画光标无对应机制）、`livenovel.text.font`（来源字体为工程外的 Windows 系统字体，无法打包注册，正文以 NIR 内置日文字体渲染）。`livenovel.text.reveal` 已升为 adapted：`StatusTextSpeed`（每字符毫秒，0 瞬时）映射为每字素簇微秒的揭示间隔，单位经固定滑条回调 `@ParamStr[0] × 64` 与官方文档双重认证。`livenovel.menu-sfx` 亦为 adapted：标题与回想网格的选择音映射为生成页面的点击效果、回想画面 BGM 映射为循环页面音乐（`ui.menu-effects.v1`），音量随 live.lpb 解码的 sfx/bgm 总线默认值；悬停参数留在 `livenovel.menu-hover` 近似中。`livenovel.textbox.fade` 同为 adapted：MESON/MESOFF 的非零渐隐毫秒映射为等时长的 dissolve 窗口揭示，零渐隐保持立即翻转；存在渐隐位点时依赖 `text.window-transition.v1`（见 [场景转场语义](STAGE-TRANSITION-SEMANTICS.md)）。`--draft` 保持自己的不完整契约，不走该门禁。

### 系统菜单解析与映射状态

LiveNovel 导出附带 `import-ui.json`，范围为该配置的系统菜单和历史回看脚本目录（报告格式 3）。报告记录实际解析字节的 SHA-256 与版本，并保留控件声明的稀疏属性编号、属性修改、表达式操作及函数编号、源位置、作用域深度、静音和延迟更新标志。声明属性编号从 1 开始，SetProp 的运行时编号从 0 开始，两者不能直接混用。

UI 报告格式 2 同时保留原始表达式和有界归一化结果（`empty` / `value` / `unsupported`）。临时变量链展开为常量、源变量读取、属性读取、数组索引及有限纯运算；整数溢出、未知调用、写入源状态和超限展开有独立诊断。非整除、混合类型和引擎状态不会被猜成常量。归一化仅在 Rust 转换器中运行，不向播放器加入源脚本解释器。

菜单条件中的 Or/And/Xor 保留为 `source_or` / `source_and` / `source_xor`，不提前决定逻辑或按位语义。Exists、IndexOfStr、Pos、AddDelimiter 保留为有限符号调用，不访问源运行时对象或执行字符串函数。这些结果用于后续经过校验的转换规则，不代表菜单条件已经绑定到 NIR 状态。

UI 数据流报告同时保留 SaveCabinet/LoadCabinet 的稀疏属性、Act 和目标数组，以及 Flip 的目标、效果、时间、方向、退出/删除、两个效果参数、来源和事件停止参数。它们描述原界面的保存/恢复及退出过程，不表示播放器已经执行了这些过程。LiveNovel 自动阅读配置另核对标准 Auto 分支的四条操作及参数；改变恢复目标、转场或启用赋值、增加操作会拒绝该配置。同时校验标准入口按标签查动作表、互异动作条件链及无条件退出，拒绝额外入口或顶层操作。该检查不证明全程序数组状态、其他分支副作用、保存容器内容或 UI 转场等价；Else 的结构性 NotUpdate 标记被允许，不据此声称刷新时序相同。

报告的 `data_flow` 另外保留 VarNew 的原变量类型/作用域/初始化值、GetProp 的读回目标、Calc 和 WhileInit/WhileLoop 的赋值及循环目标。能识别的单次赋值记录目标与纯值表达式；未知调用、数组写入和复合副作用记录 `unresolved`，不默认视为无作用。该列表尚未做跨语句常量传播，也不等于完整控制流或所有副作用清单：调用、跳转等仍需后续分析，不能据此把此前变量值跨过未知操作继续使用。

每条声明/修改附带 `guards`：If/Elseif/Else 的正反条件、循环条件及源索引。`excluded_by_constant_branch` 只表示词法分支条件排除了该声明，不是考虑跳转、调用和循环状态后的全程序不可达证明。静音条件不会用于此排除结论；源对象仍保留在报告和未映射诊断中。布局里依赖 `GetProp` 的尺寸、鼠标坐标、动态数组与分支数据流仍需后续映射。

当前状态明确为 `parsed_not_lowered`：这些系统页尚未生成 NIR UI。每个未映射的非静音对象在 `import-report.json` 中报告源位置；滑条同时报告可解析的原始边界和步长。解析失败单独报告，状态为 `partially_parsed_not_lowered`；目录没有对应脚本则为 `not_present`，均不当作已支持。报告不是源脚本执行器，也不证明分支可达、布局求值或回调迁移已经完成。

LiveNovel 另导出 `import-menu-items.json`，从原初始化脚本的字面量 StringToArray 提取菜单项名称/动作两组数组，按原顺序配对，记录来源哈希和写入位置。动作以源标识为准，不根据显示文字猜测。当前仅识别有界、显式逗号分隔、非空元素的直接初始化；长度不一致、重复标签/动作、重复写入或动态初始化明确拒绝。源 NotUpdate 标记保留，不因该标记否定声明提取。

菜单项报告状态为 `declarations_extracted_not_lowered`：它不证明初始化调用一定可达、后续无人修改数组、回调已经迁移或页面可以播放。与详细 UI 报告一样，它含原工程文字，不应加入公共测试或源码仓库。

`inspect` 仅增加控件/属性的数量统计，不输出控件名字、原始表达式、图片路径或正文。导出工程的详细 UI 报告含来源信息，应与原工程一起保留，不能当作公共测试素材。

## 通用 LSB 路径与诊断

| 内容 | 当前行为 |
| --- | --- |
| Label、Comment、静音指令 | 保留控制流位置；注释和静音指令不执行 |
| Jump、Call、Exit | 支持字面量整数/Flag 条件、跨文件跳转和无参数调用、非零 Label 调用与返回 |
| Terminate | 结束当前 NIR 作品 |
| 同步 TextIns | 支持常量目标、启用历史、Wait 和 StopEvent 为真；普通字符、换行和分页转换为 NIR 对白 |
| 文本样式、原消息框、揭示速度 | 使用 NIR 阅读器呈现；报告明确记录警告 |
| 动态表达式、变量、结构化条件/循环、带参数调用 | 报告阻塞项 |
| 异步文字、事件、选择菜单、ruby、交互链接、条件文字 | 报告阻塞项 |
| 场景、GAL 图像、音频、原引擎菜单与存档 | 通用路径仍报告阻塞；上方 LiveNovel 配置单独适配 |

转换仅沿入口的可达控制流进行。遇到未支持指令后，该路径的分析停止；报告不宣称覆盖后续内容。可先用 `inspect` 查看全包结构。

默认严格模式遇到阻塞项不生成目录，JSON 报告写到 stdout，命令以非零状态退出。报告保留源文件、指令索引、LineNo、字节偏移和 NIR ID 映射。通用路径同样携带映射账本：`lsb.control-flow`（exact）、`lsb.text`（adapted，正文排版由 NIR 阅读器呈现、媒体仅报告不转换）；阻塞时另有一条 `lsb.unsupported-commands`（unsupported）汇总未支持位置。

```sh
./novelc import livemaker "/path/to/extracted-game" --out migration-draft --draft
```

`--draft` 用于人工移植：未支持指令生成 `E_IMPORT_UNSUPPORTED` 故障块，附 `MIGRATION-INCOMPLETE.txt` 和 `import-report.json`，仍以非零状态退出。草稿不是已经移植完成的可玩游戏，也不会静默跳过未知指令。

## 开发与验证

```sh
cargo test -p nir-compiler import --lib
cargo xtask sdk
python3 scripts/verify_sdk.py
```

Rust 测试自行构造无游戏素材的 LSB，覆盖日文解码、截断输入、跳转与调用、源位置、严格/草稿模式和生成作品的实际剧情执行。SDK 验证将 CLI/SDK 复制到独立目录并清空 PATH，验证导入不调用外部工具。

二进制格式参考：[pylivemaker 的 LSB 文档](https://pylivemaker.readthedocs.io/en/latest/livemaker.lsb.html)。pylivemaker 仅用于开发阶段的结构对照，不是构建或运行依赖。

可选的本地游戏包验证（不把游戏正文、图片或声音加入测试仓库）：

```sh
NIR_IMPORT_SOURCE="/path/to/extracted-game" \
NIR_IMPORT_OUT="/path/to/new-project" \
cargo test -p nir-compiler real_livenovel_conversion_and_all_routes -- --ignored --nocapture
```

该测试导入实际包，自动推进主线和所有回想入口，检查故障、结束、解锁和快照恢复。正常测试使用自行生成的中性 LSB、GAL、WAV 样本与菜单配置，覆盖格式边界、透明度、块引用、声道转换、菜单缩放和锁定入口。

完整的 Player 级路线认证（阅读/界面/存读档/回想事务走共享播放器，而非 VM 直驱）：

```sh
NIR_IMPORT_SOURCE="/path/to/extracted-game" \
NIR_IMPORT_OUT="/path/to/new-project" \
cargo test -p nir-compiler real_livenovel_player_certifies -- --ignored --nocapture
```

认证内容：Auto 自动阅读走完主线并集齐回想解锁；未解锁档案下回想入口经真实菜单控件分发时被拒绝，不切换会话也不离开菜单；经作者菜单控件进入全部回想入口，读完经 return-to-title 结果返回回想菜单；已读主线在按住快进下整线快进；隐藏（默认继续政策）与恢复不推进；菜单暂停/关闭恢复同页；演出中保存→前进→回退→槽位读档恢复保存页后走完全程。

新导入的 LiveNovel 页首及页内 Gate 事件后会绑定具体非循环 Voice 实例，Auto 不再被无关语音阻塞；逻辑页、视口翻页和并行等待的边界见 [阅读语义](READING-SEMANTICS.md)。


`import-defaults.json` 记录 LPB 输入哈希、选取的音量/等待/字速值（`text_speed_ms`，每字符毫秒），不复制作者工程目录、项目标题或全部系统设置。来源未知类型、缺失或越界的必要值会报错，不能无提示地替换成模板默认值。LPB116 后续编辑器数据区保留为未解释部分，不执行、不导出。

### 原系统菜单草稿

识别上述 LiveNovel 配置时，`--draft` 另生成不完整的系统菜单覆盖页；普通导入暂不启用。标签和顺序来自原名称／动作表，位置、字号、行距与文字颜色来自原 Menu 声明。已校验分派与分支体的 Auto／已读快进／隐藏文字动作绑定阅读服务；历史动作在来源校验后进入生成的子页。绑定依据动作 ID，不依据显示标签；其可用性由播放器再次检查。未映射的项目以灰色静态文字呈现，不携带占位动作。Escape 使用播放器的逐层关闭服务。

该草稿尚未迁移其余原显示条件、子菜单、容器／截图恢复和 200 ms 退出动画；字体替代、字号／行距单位、固定行宽和半透明变暗只是预览近似。`import-menu-preview.json` 格式 2 记录来源哈希、绑定、限制及历史页报告引用；`import-report.json` 状态为 `incomplete_ui_preview`，附错误诊断与 `MIGRATION-INCOMPLETE.txt`，工程验证后写出，命令仍以非零状态退出。它是转换链路的中间验收产物，不是原菜单完整兼容的声明。

菜单草稿现也校验已读快进分支的容器恢复、退出参数、三个跳过标记及两项消息框属性，并核对原菜单中的已读／无选择／非回想条件。匹配后绑定 SkipRead 服务：未读、选择期间和回想中隐藏该项。转换器为主线／回想包装入口生成 bool 上下文，使用 `ui.menu-story.v1` 显式只读导出；该字段按现有故事快照保存恢复。尚未证明原菜单初始化的全部控制流、数组全程序不变性及原设备时序，草稿不完整状态保留。

隐藏文字草稿映射到 PeekStory：校验源分支的局部可见性缓存、组件列表筛选、三段循环跳转、五种恢复输入、两次 Flip 参数及最终菜单恢复，保留故事暂停和同一菜单实例。恢复输入被消费，不推进对白或选择。编译器不会运行 ListCompo 或源等待循环；其输出数组和槽位清空分别作为显式副作用形状校验。普通纯表达式路径拒绝重复临时目标，防止把数组别名写入误认成纯读取；IsDelimiter 仅增加有界符号表示，不执行源函数。

源 `#` 开头的组件属于此隐藏分组；若被转换为普通 Story 节点，当前草稿明确拒绝，待具有对应 UI 投影后再映射。原 200 ms 渐变、任意源 UI 组件和截图／容器内部状态仍不作等价声明，因此导入继续输出不完整诊断。

历史分析保留 `CallHist` 的 target/index/count/cut_break/format_name，以及 `FormatHist` 的 name/target、原始表达式、条件和来源位置。参数保留来源单位，不能直接当作 NIR 的历史条目 offset/limit。普通导入仍对活动历史调用报告 `E_IMPORT_UI_HISTORY_UNMAPPED`；原始 UI 分析报告保持 `parsed_not_lowered`，不把解析结果当作生成页面。

草稿历史页校验已知 116 配置的完整页面命令结构、格式器注册、初始样式、源选择／滚动／滚轮回调和 Escape 删除目标。格式器已注册时 `@HistoryCount` 表示排版文字高度，不能当作条目数量。轨道为两个横向状态、滑块三个、箭头六个；按文档顺序拆分为原尺寸 PNG，边框按原像素平铺。来源 ScrollbarHeight 只表示轨道高度，生成控件的总高度另外包含两端箭头。尺寸不合、动态样式或未知操作直接拒绝，不猜测回调和状态顺序。

生成页使用 `HistoryFlow`、图片滚动条和 `push_menu`，首次定位最新端；窗口、字号／行距、滚轮及翻页步长来自已检查的来源关系。该页声明 `builtin_navigation=false`，避免 NIR 自动返回按钮遮住原上箭头；根菜单历史项要求消息框可用且历史非空。Escape 返回父菜单，旧页面输入失效，剧情保持暂停。`import-history-preview.json` 记录输入脚本／图片哈希、几何、状态顺序依据和限制。只在该页校验并写出后，将对应来源诊断改为 `E_IMPORT_UI_DRAFT_ADAPTED`，其他页保持未映射诊断。

目前使用 NIR 的有界历史记录和字体排版，尚未复刻来源 scenario-page 间隔、按格式器页数保留历史、字体阴影／描边、动态源样式、箭头按住重复及精确轨道拉伸。原作可执行文件对照尚未完成，报告和命令继续明确标记不完整，不能把原图已复用当作完整历史语义兼容。
