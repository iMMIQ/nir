# 字体编译与独立创作模板

本阶段落实 NIR-0006 的作者工程/资源流水线、NIR-0003 的字体覆盖和塑形闭包，以及 NIR-0004 的派生资源缓存的一部分。文本修改后的修订/复核步骤见 [翻译维护](TEXT-REVISIONS.md)。作者使用配套 `novelc` 和 SDK 即可写新中文，无需 Rust、Python、fontTools 或系统 HarfBuzz。已发布的 v0.1.0 下载包尚不包含此能力；先从本分支源码构建 SDK。

## 创建作品

```sh
./dist/novelc init my-story
./dist/novelc -p my-story resolve
./dist/novelc -p my-story check --locked
./dist/novelc -p my-story test
./dist/novelc -p my-story dev
./dist/novelc -p my-story build --locked
```

`init` 默认使用 `minimal`：一个模块、双语对白、一个选择、两个结局、两条用例、可编辑主题和完整 Noto Sans CJK SC Regular 2.004 母版字体。每次创建生成独立 GameId。没有预设人物、背景图或音频。正文位于 `content/main/texts/`，逻辑位于 `content/main/story.nir.json`，字体位于 `assets/fonts/`。

`init --template web-basic` 保留《雨后书简》回归模板，适合研究场景、音频、Gate 和恢复。其旧字体仍是固定子集；要自由增写中文，替换为母版并加入下述配置。测试工程与引擎产品的定位不变。

## 资源配置

```toml
format = 1
[[assets]]
id = "font.reader"
kind = "font"
source = "fonts/NotoSansCJKsc-Regular.otf"
rights = "OFL-1.1; see fonts/SOURCE.md"
[assets.font]
mode = "subset"
face_index = 0
extra_characters = ""
license = "fonts/OFL.txt"
```

`source` 与 `license` 相对 catalog 所在目录；必须为工程内文件，不允许 `..`、绝对路径或越界符号链接。`license` 为非空 UTF-8 许可文本，自动汇入发行 `NOTICE.txt` 和哈希许可对象。作者仍须选用允许相应处理和分发的字体，并保留权利人声明；工具不推断授权。母版来自何处、版本和哈希可以记录在 SOURCE.md，并登记到 `inputs.notices`。

- `mode = "subset"`：保留作品需要的字符，以及字体塑形需要的派生字形。
- `mode = "full"`：保留所选 face 的全部字符和字形，仍重新生成单 face 字体。适合无法枚举的动态内容，但不等于可显示任意 Unicode，也不保留未支持的字体表。完整 CJK 字体会增加下载与运行内存。
- `face_index`：默认 0，可从 TTC 集合选择 face，输出为单 face OTF 或 TTF。
- `extra_characters`：明确预留未来动态文本的字符，必须由这份母版覆盖。

无 `font` 表的旧字体资源继续直接打包，但也执行完整的已知文本覆盖检查。未知配置、未知模式、错误 face 和不支持的字体能力均报错。当前只接受静态 OpenType `glyf`/CFF 轮廓；不接受可变字体、彩色/位图字体、AAT/Graphite 排版或 WOFF。`assets.schema.json` 提供编辑器字段提示。

## 覆盖与排版

编译器收集所有正文语言的 TextDoc、标题、对白角色字段、文本 ID，以及所有类型化 String 常量，包括初始值、Assign、函数实参等嵌套表达式。ASCII 用于数字、Bool、错误码和固定控件；界面字符直接取自 SDK 内嵌的两份 Fluent 消息（包括固定控件符号）。多个字体共同覆盖正文/UI；额外预留字符按各自字体检查。缺字报 `E_FONT_COVERAGE`，列出字符及 Unicode 码位，最多展示前 12 项。

子集由 CLI 静态链接的 `hb-subset 0.3.0` / HarfBuzz 8.2.2 生成，保留所有布局 script/feature 的 GSUB、GPOS、GDEF 闭包和名称/许可信息。额外补齐 Unicode 规范分解与重组可能使用的字符，避免 `a + combining acute` 在裁剪后失去原先的组合字形。正文、Span 与 Param 的内容不被改写。输出再次验证字体格式与覆盖，按真实签名选择 `font/otf` 或 `font/ttf`。

这不是完整的 LocaleFontPlan：尚未提供作者可配置的逐语言 fallback 顺序、独立塑形 locale、RTL/Ruby 认证或运行时补字下载。任意外部输入、未来存档迁移和玩家姓名输入不在当前功能中；不可枚举的字符应预留或使用 full，并自行验证目标脚本。覆盖检查不等于所有字体、语言和字号均完成视觉认证。

## 缓存与发布

`.nir/cache/fonts/` 的内容键包含源字体 SHA-256、工具/Unicode 数据版本、流水线实现身份、模式、face 和所需字符集合。没有时间戳或绝对路径。GSUB/GPOS 闭包由输入字符和字体内容确定。字体产物带摘要封装，以临时文件原子提交；损坏缓存重新生成。缓存目录和条目拒绝符号链接。可随时删除 `.nir/cache/fonts/`，随后相同输入生成相同内容对象。

改图片不改变字体键；改字集会产生新的字体键；相同字集的正文改写可复用字体。许可修改不会迫使重新裁剪字体，但会改变许可对象和发行身份。`game.lock` 继续锁定完整 SDK 和 CLI，`--locked` 禁止工具漂移。没有实现跨项目缓存、缓存容量淘汰或通用增量构建图。

`reports/build.json` 的 `fonts` 给出每个运行字体的工具版本、源/输出大小与摘要、所选 face、请求/保留字符数、cache_key/cache_hit、许可路径及摘要。报告是作者侧数据，缓存命中情况不会影响发行身份。源母版、缓存和构建报告不作为运行资源复制到静态站点；运行目录只含生成的字体对象及登记的许可说明。

开发预览自动监测母版、许可和正文修改。准备失败继续展示上一发行和当前会话，修复后生成新字体并完整重载。不是运行会话的安全热更新。

## 用例与构建

新用例只需检查 `expect.outcome`，可用类型化 `expect.variables` 检查自己的变量，例如：

```toml
[expect]
outcome = "garden"
[expect.variables.visited]
type = "bool"
value = true
```

旧 `expect.affection` 保留兼容，只有显式填写时才检查，且允许与新断言同时使用。

构建引擎的机器额外需要 C++11 编译器与 libclang（例如 Ubuntu 的 `g++ libclang-dev`）。HarfBuzz 随锁定 Rust 依赖静态编译进原生 CLI，不依赖机器安装的 HarfBuzz，不进入播放器/WASM。SDK 第三方清单包含其上游 COPYING。更新此依赖时需重新验证静态字体样本、塑形一致性、字形闭包和产物身份。

实测范围和结果见 [本阶段验证](validation/author-fonts/README.md)。
