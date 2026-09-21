# 雨后书简 · Rain Letters

约五分钟的原创双语交互故事。雨夜、旧站台、一封迟到的信；选择一同走回去，或留下来读完它，进入不同结局。

在本目录运行配套 `novelc`：

```sh
novelc resolve
novelc check --locked
novelc test
novelc dev
novelc build --locked
```

`novelc` 与 `sdk/` 应放在同一目录，或用 `--sdk` 指向配套 SDK。编辑正文从 `content/ch01/texts/` 开始，逻辑从 `story.nir.json` 开始。`tests/scenarios/` 含两条路线的预期结局。

素材由确定性程序生成；测试声音不是配音。新增字形必须更新字体，完整许可在 `credits/`。完整源仓库中的 `scripts/make_fixture.py` 会重建整份示例与核心测试 fixture。
