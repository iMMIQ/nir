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

提供受限的 **LiveNovel 116 线性剧情／回想配置**。从 `ノベルシステム/START.lsb` 启动、带 `menu.lpm` 和回想分发脚本时自动识别。适配器读取实际主线跳转、回想目标、解锁 ID 与 LPM 按钮；替换标准系统脚本。当前配置的舞台、消息框和系统资源约定为 1024×768，不代表所有 LiveNovel 作品都能直接导入。未知事件或不匹配的剧情结构会报错，不会静默忽略。

- `CREATECG`、`CHANGECG`、`DELETECG`：图层、位置、叠放顺序、纯色画面；替换不存在的图层时按原约定创建。原 wipe 用相同时长的 dissolve 替代。
- `PLAYSND`、`STOPSND`：BGM 循环、语音替换、原音量；WAV PCM16 与 Ogg/Vorbis 转成播放器支持的 WAV，六声道 WAV 下混成立体声。目前停止淡出是立即停止；事件音量预乘到 PCM16（超出满刻度会截断），系统音量偏好使用 NIR 默认值。
- `WAIT`、`MESON`、`MESOFF`、消息结束握手：顺序等待和消息框显隐。页内事件使用 NIR Gate 保留在文字中的执行位置；消息框淡入淡出目前以立即显隐替代。
- 标题菜单：原背景、按钮普通／悬停图片和 LPM 坐标，等比例适配视口，指针点击与 Web 键盘／辅助语义。
- 回想：原缩略图与网格坐标、独立入口、Profile 解锁，读完返回回想菜单。锁定项变暗并拒绝执行；另保留 NIR 系统菜单和返回按钮，便于触摸访问。
- 对白：原消息框图片、位置、透明度、白色文字、32 像素基础字号和 40 像素行高。字体采用随 SDK 提供的字体，揭示间隔为 32 ms。

GAL 105/106 支持有界的单帧 8/24/32 位图、原始／zlib 数据、块引用、透明度、调色板、图层合成与尾部矩形列表。动画 GAL、LCM 视频和归档解包尚未支持；本配置不会导入未引用的动画光标。解码由 Rust 在同一个 `novelc` 内完成，没有外部媒体进程。

**尚未等同于原引擎的部分：** 存读档、设置、历史记录使用 NIR 系统界面；旧 LiveMaker 存档不兼容。原 wipe、声音／消息框渐变、回想菜单音乐、菜单音效、动画光标、逐字符原字体样式尚未完整复刻。报告状态为 `converted_with_adaptations`，逐项列出这些差异；不是无差异转换认证。

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

默认严格模式遇到阻塞项不生成目录，JSON 报告写到 stdout，命令以非零状态退出。报告保留源文件、指令索引、LineNo、字节偏移和 NIR ID 映射。

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
