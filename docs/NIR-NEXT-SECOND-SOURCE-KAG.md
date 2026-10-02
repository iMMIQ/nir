# NIR-NEXT 第二来源认证记录：KAG / KiriKiri2（P0.1 收口）

对应实施计划 §4 P0.1：「来源样例至少两个引擎家族。优先现有 LiveNovel 适配和 KAG 的小型固定子集；第二来源不要求先实现完整导入器。记录精确版本/源码提交/参数/证据方式；无原版运行证据时标『待认证』。」本记录只做规格级认证，不交付 KAG 导入器；在导入器实际交付并经各自门禁验收前，任何 KAG 支持声明均为「待认证」，不写入仓库其他文档。

第一来源（LiveNovel）的真实语料映射账本与全路线认证见 [IMPORT.md](IMPORT.md) 及本地 reports/；本记录登记第二来源（KAG）的版本、提交、参数与证据方式。所引 KAG/KiriKiri 均为公开许可的源码项目，本仓库不复制其源码或素材，只登记身份与行为语义。

## 1. 引擎家族与精确版本

| 项 | 值 | 证据锚点 |
| --- | --- | --- |
| 引擎家族 | KiriKiri2（吉里吉里2）＋ KAG3 场景系统；与 LiveNovel 的 LSB116 专有字节码家族相互独立（差异见 §4） | — |
| KAG 版本串 | `3.32 stable rev. 2` | krkrz/kag3 `data/system/Initialize.tjs:5`（`var kagVersion = "3.32 stable rev. 2";`） |
| KAG 系统层源码 | github.com/krkrz/kag3 @ `1f3ab309106d210e3169bbbe0fb4e066ae463b42`（2017-12-24，「KAG3 for 吉里吉里Z」维护线，自报版本即上表） | 仓库 HEAD |
| 场景解析器源码 | github.com/krkrz/KAGParser @ `c2269b26b390bd09aa1f2b39d7d779cb79c26f2e`（2024-09-17，KAG スクリプトパーサープラグイン） | 仓库 HEAD |
| 引擎稳定线 | KiriKiri2 2.32 stable rev.2（最终稳定版；发布包 kr2_232r2.zip，2010-10-26） | krkrz/krkr2 `kirikiri2/branches/2.32stable` 头部 `9c892a7fa773b66077369bb85fc9637906a71907`（2010-10-26T08:28:03Z，提交信息「2.32stable2コミット」，与发布包同日同名） |
| 引擎归档镜像 | github.com/krkrz/krkr2 master @ `dec49af97e174d31059c3ccd7efc700ba3c6b788`（2017-12-31，GPL-2.0，SVN 镜像「吉里吉里2過去ログ」） | 仓库 HEAD |
| 许可 | krkr2：GPL-2.0；kag3 TJS 文件头：「Copyright (C)2001-2009, W.Dee and contributors 改変・配布は自由です」；KAGParser.cpp 文件头：「Copyright (C) 2000 W.Dee and contributors」（许可细节随仓库 license.txt） | KAGParser.cpp:4、Conductor.tjs:2、MainWindow.tjs:2 |
| 官方文档 | krkrz.github.io/krkr2doc/kag3doc/（KAG3 HTML Help 镜像，仓库 krkrz/krkr2doc「吉里吉里2ドキュメントミラー」） | 仓库 krkrz/krkr2doc |

线体说明：kag3@master 是面向吉里吉里Z 的维护线（少量修正与 UTF-8 化），自报版本串 3.32 stable rev. 2；2.32 时代经典 KAG3 的解析层随包以 TJS 提供，Z 线并入 KAGParser 插件（kag3 系统层经 `class BaseConductor extends KAGParser` 继承，Conductor.tjs:13）。本记录冻结的小型固定子集在两线语义一致；行号锚点取自上述公开仓库当前 HEAD。

## 2. 小型固定子集（规格冻结）

子集取自 KAG 场景（.ks）文法，覆盖正文、等待、标签、跳转、条件、选项链接与终止，构成第二引擎家族的最小可认证面。仅使用方括号标签形式（KAG 另有行首 `@` 引入标签的形式，KAGParser.cpp:1092-1094 注释并列举两者，不进入本子集）。语义逐项锚定公开源码：

| 语法 | 语义 | 参数 | 源码锚点 |
| --- | --- | --- | --- |
| 文本行 | 逐字绘入当前消息层；行首 `;` 为注释行；`[[` 转义为字面 `[` | — | KAGParser.cpp:1105-1106（`;`）、:1491-1493 与 :1539（`[[`） |
| `[l]` | 行クリック待ち：正文在该处停住，等待点击/按键后继续（showLineBreak） | 无必需参数 | kag3 MainWindow.tjs:4977 |
| `[p]` | ページクリック待ち：等待点击/按键后清页继续（showPageBreak；历史启用时先 reline） | 无必需参数 | MainWindow.tjs:4983 |
| `*名称` | 标签定义；可带 `|页名` 后缀；解析期登记进标签缓存 | 名称（`|` 后页名） | KAGParser.cpp:1108-1120（含 RecordingMacro 下的拒绝） |
| `[jump]` | 无条件跳转到指定场景文件的指定标签 | `storage`（目标 .ks，缺省当前文件）、`target`（`*标签`） | KAGParser.cpp:1613-1614（特殊标签表）；Conductor.tjs:499（onJump） |
| `[if]`/`[elsif]`/`[else]`/`[endif]` | 条件包含式分支：`exp` 求值选取包含区间 | `exp`（TJS 表达式） | KAGParser.cpp:1583-1602（tag_if/tag_else/tag_elsif/tag_endif 分发表）、:1758-1759 |
| `[link]`…`[endlink]` | 区间文字成为可选链接；命中后求值可选表达式并跳转：onMouseDown → findLink → processLink → `window.process(storage, target, countPage)` | `storage`、`target`（跳转目标）；可选 `exp`/`clickse`/`clicksebuf`、`hint`、`color`（缺省 defaultLinkColor）、`opacity`、`onenter`/`onleave`、`countpage`（缺省 true） | MainWindow.tjs:4888/4896 → MessageLayer.tjs:1640 beginHyperLink（参数字典逐项）、:1664 endHyperLink、:2163 onMouseDown、:1991 processLink |
| `[s]` | 実行停止：进入稳定停点（inSleep、notifyStable），场景终态 | 无必需参数 | MainWindow.tjs:5116 |

解析与执行分层（结构事实）：场景解析、标签缓存与条件包含由 KAGParser 承担（`class BaseConductor extends KAGParser`，Conductor.tjs:13；goToLabel/onLabel 见 Conductor.tjs:317/494）；执行期标签由 kag3 系统层的处理器字典承担（MainWindow.tjs 内 `名前 : function(elm)` 形式）。

## 3. 证据方式与认证状态

| 项 | 状态 |
| --- | --- |
| 证据类 | documented：公开源码（精确提交）＋官方文档镜像；不涉及解码私有二进制。与 LiveNovel 侧证据类（documented/decoded-source，见 [IMPORT.md](IMPORT.md)）并存 |
| 原版运行对照 | **待认证**：未取得 KiriKiri2 2.32 stable（kr2_232r2.exe）或吉里吉里Z 实机的运行记录；计划 §1 不授权运行未知原版可执行文件。在此之前不声明任何「与原版一致」 |
| 导入器 | 未交付（计划原文「第二来源不要求先实现完整导入器」）；本记录不构成 KAG 支持声明 |
| 子集稳定性 | 冻结于 §2；后续 KAG 相关工作以本子集为界，超出子集的标签须按导入账本规则单独认证 |

## 4. 与 NIR 概念的对应（映射笔记，非交付承诺）

- `[p]` ≈ 源页边界（Await Advance）；`[l]` ≈ 页内等待的更细粒度（行等待）；`[s]` ≈ 函数终态。
- `*标签`/`[jump]`/`[if]` ≈ 块图控制流：标签对应块入口，jump 对应跳转边，if 对应条件分支。
- `[link]`…`[endlink]` ≈ Interact 选项：KAG 链接点击后按 target 直接跳转（选项=跳转目标），与 LiveNovel 選択値 约定（批次 52：提交值→字面量分发）同构——两个家族的选项机制都能降级到 `story.typed-result.v1` 同一核心，这是 P5 关卡「第二来源复用相同核心」在 KAG 侧的对应面。
- 家族差异佐证：KAG 是明文场景文法（解析期标签缓存/宏/条件包含）＋TJS 宿主语言；LiveNovel 是专有二进制操作码流（LSB116）。解析层、选项机制、等待语义的实现路径均不同，满足「至少两个引擎家族」。

## 5. 复核方式（公开网络可复现）

```console
# KAG 版本串
$ curl -s https://raw.githubusercontent.com/krkrz/kag3/master/data/system/Initialize.tjs | sed -n '5p'
var kagVersion = "3.32 stable rev. 2";
# 引擎稳定线头部（提交信息与日期即 2.32stable2 发布）
$ curl -s "https://api.github.com/repos/krkrz/krkr2/commits?path=kirikiri2/branches/2.32stable&per_page=1"
# 解析器与系统层 HEAD 及锚点
$ curl -s "https://api.github.com/repos/krkrz/KAGParser/commits?per_page=1"
$ curl -s https://raw.githubusercontent.com/krkrz/kag3/master/data/system/MainWindow.tjs | grep -n "^	l : function\|^	p : function\|^	s : function\|^	link : function"
$ curl -s https://raw.githubusercontent.com/krkrz/KAGParser/master/KAGParser.cpp | grep -n "elsif\|\"jump\"\|TJS_W(';')\|TJS_W('\*')"
```
