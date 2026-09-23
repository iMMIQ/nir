# 从这里开始写作品

这是独立创作模板，包含简中/英文对白、一个选择和两个结局。没有预设角色、图像、声音或示例作品的变量。

1. 修改 `game.toml` 的标题和 slug；`init` 已生成独立 GameId。
2. 编辑 `content/main/texts/zh-Hans.json` 和 `en.json` 的 `text`。两种语言保留相同的文本 ID、修订、参数与 Gate 契约。
3. 编辑 `content/main/story.nir.json` 的函数块、Cue 和选择；新增文本时同步更新 `texts/contracts.json`。
4. 在 `config/locales.toml` 分别选择 UI/正文默认语言与每种语言的字体顺序；用 `theme/theme.toml` 和 `tokens.json` 调整内置 UI。PNG 和 PCM16 WAV 可登记到 `assets/catalog.toml` 后从剧情引用。
5. 修改源文后运行 `novelc -p . text update --id intro --meaning preserve`（含义或契约变化用 `bump`），检查并修改英文后运行 `novelc -p . text review --id intro --locale en`。用 `text status` 查看所有待复核项，`texts/revisions.json` 纳入版本管理。
6. 执行 `novelc -p . resolve`，然后 `check --locked`、`test`、`dev` 或 `build --locked`。

默认语言配置使用简中界面/正文和英文界面/正文，四个计划都引用 Noto Sans CJK SC 母版；CLI 按每个计划分别检查双语正文、界面、标题与已知字符串常量，并把共享字体所需字集合并后生成一个运行子集。设置中可组合不同 UI 与正文语言。新增中文通常只需编辑正文。源字体不支持的字符会报 `E_FONT_COVERAGE`，需补充有许可的字体。动态预留字符填写 `assets.font.extra_characters`；`mode = "full"` 保留源字体全部字符，会显著增加下载和内存占用。

`.nir/cache/fonts/` 是可删除的派生缓存；`reports/build.json` 包含字体来源、工具、哈希、大小和缓存结果。字体母版和许可应纳入作者版本管理；不要将整个作者目录当作发布目录。发布 `dist/full/web/` 并保留 `NOTICE.txt`。

随 SDK 的文档 `docs/AUTHOR-FONTS.md` 和 `docs/LOCALE-FONTS.md` 说明字体配置、格式和边界。原创模板内容使用 CC0-1.0；字体使用 OFL-1.1；运行引擎使用 LGPL-3.0-or-later。
