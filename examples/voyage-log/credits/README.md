# 素材来源

背景与按钮由仓库 `scripts/make_p0_examples.py` 确定性原创生成，以 CC0-1.0 提供；声音为合成测试音，不是真人配音。

字体子集来自仓库 SDK 模板的 Noto Sans CJK SC Regular 2.004 母本
（`templates/minimal/assets/fonts/`，SIL OFL 1.1，附完整许可）。`assets/source/reader.chars.txt`
列出该字体必须覆盖的字符；子集用与编译器相同的 vendored HarfBuzz 工具链生成
（见仓库 `docs/NIR-NEXT-P0-BASELINE.md` 的重建步骤）。新增文字时必须补充字体覆盖。

本工程是 NIR-NEXT P0 验收样例；不含任何第三方游戏内容。
