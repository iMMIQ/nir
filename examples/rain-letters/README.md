# 引擎测试工程：雨后书简 · Rain Letters

这是 NIR 引擎的测试工程，用于验证简中 / 英文正文、选项分支、场景演出、音频、存读档、检查点回退和回看，并演示作品工程格式。两个结局对应两条剧情测试路线；图像和声音采用简单的测试素材。

测试剧情以雨夜旧站台和一封迟到的信为背景，选择一同走回去或留下读信，进入不同结局。

在本目录运行配套 `novelc`：

```sh
novelc resolve
novelc check --locked
novelc test
novelc dev
novelc build --locked
```

`novelc` 与 `sdk/` 应放在同一目录，或用 `--sdk` 指向配套 SDK。编辑正文从 `content/ch01/texts/` 开始，逻辑从 `story.nir.json` 开始。`tests/scenarios/` 含两条路线的预期结局。

`config/locales.toml` 分别配置界面与正文语言字体：简中使用 Noto Sans CJK，英文先用 ABeeZee、缺字时使用 Noto。设置页可以独立切换 UI 和正文；已打开的对白、选项与历史保持原语言和字体计划。具体行为和限制见仓库文档 `docs/LOCALE-FONTS.md`。

## 播放器操作

- 空格 / Enter：开始、揭示至 Gate 或段尾；长文先向下翻页，到底后继续剧情。Esc：菜单 / 返回。
- 鼠标或触摸：点击对白区、选项和绘制的按钮。长内容可用滚轮、上下滑动、PageUp / PageDown 或画布翻页按钮浏览；翻页不会释放 Gate。
- Tab、方向键与 Enter：访问辅助语义按钮；可见焦点框对应画布按钮。
- 菜单提供回看、检查点回退、设置和三个存档槽。读档和设备恢复后先暂停，按继续恢复。
- 「已读快进」只推进已读正文，遇到未读段落或选项停止。自动阅读等待正文及当前语音完成。
- 声音须首次点击 / 按键解锁。`voice.wav` 是合成测试声。

素材由确定性程序生成；测试声音不是配音。新增字形必须更新字体，完整许可在 `credits/`。完整源仓库中的 `scripts/make_fixture.py` 会重建整份示例与核心测试 fixture。
