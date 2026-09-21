# NIR-0003：国际化、内容分包与 Web 发布

状态：架构提案 v0.1；日期：2026-09-20。承接 NIR-0001 与 NIR-0002。

适用范围：Rust/WASM 运行时，wgpu 渲染，Web 首版；不涉及旧引擎转换、作者工作台或原生安装包。本文的配置字段、模块名和发布协议是建议设计，不是已经实现的引擎 API。附件测试仅验证样例的内容寻址和局部语义契约，不验证字体塑形、Rust 编译、浏览器渲染或实际部署。

## 0. 结论

国际化与发布共同管理「同一个剧情程序，在某个固定发行版本里，使用哪些相容的文本、字体、语音和图像」。推荐：

1. 一个语言无关的 NIR 程序；界面文案、正文、语音、含字图像和字体独立分包。
2. LocaleResolver 选择语言；LocalizationResolver 解析类型化本地化内容；PrepareCoordinator 做完整准备；只有安全边界才能提交语言上下文。
3. 一个不可变 ReleaseManifest 锁定引擎和全部内容包的精确对象身份；会话只选择其中的子集，不查询各组件的 latest。
4. 哈希对象先上传，清单随后，发布入口最后切换；旧会话继续使用旧图，而不是运行时拼装新旧版本。
5. 在线静态发布为 P0；显式章节离线与 PWA 为 P1。ZIP 是交付容器，不是浏览器必须下载并解压的游戏运行包。

## 1. 必须保持的不变量

- 改界面语言不修改剧情变量、PC、已选 OptionId、随机状态或可用选项集合。
- 文本语种和配音语种独立。格式化区域只影响显示，不能改变日期的实际时间或剧情判断。
- 翻译是受限数据，不能包含任意脚本、网络请求、变量赋值或隐藏剧情跳转。
- 语言选择不依据下载速度竞争，不允许语言包到达顺序决定最终语种。
- 所有语言的语义 Gate 契约一致；允许的文本长度、样式和呈现停顿不同，不能擅自增加或删除剧情副作用。
- 网络失败、语言缺失、资源损坏是不同错误，不能把它们都处理成自动回退英文。
- 一次会话固定 release digest；同发行版本内尚未下载的语言包可以补下，新发行版本的语言包不能被静默接入。
- 回退／读档恢复旧对白实例的实际内容；不按今天的翻译和当前名字重新生成历史事实。
- 资源淘汰不删除应用认为不可丢失的存档；仍须提供浏览器清理后的备份恢复途径。

## 2. 语言上下文

建议数据：

```text
LocalePreferences（玩家偏好）
  ui_locale_requested
  text_locale_requested
  voice_locale_requested
  formatting_locale_requested

ResolvedLocaleContext（当前执行上下文）
  release_digest
  ui_locale_resolved + ui_bundle_digest
  text_locale_resolved + text_bundle_digest
  voice_locale_resolved + voice_pack_digest | none
  formatting_profile + data_profile_digest
  font_plan_digest
  ui_epoch / text_epoch / voice_epoch / typography_epoch
```

requested 和 resolved 必须分开。用户想要一种语言，不等于当前内容已经提供、下载或应用了它。界面要能显示「已选择，下一段生效」「未下载」「当前章节未提供」「下载失败，仍使用原语言」。

### 2.1 标签与协商

语言使用 BCP 47，例如 `zh-Hans`、`zh-Hant`、`ja`、`en`。BCP 47 包含语言、书写系统、地区等子标签，不能仅用两个字符存储语言。[R1]

首次优先级：明确设置 > 该作品已保存的偏好 > navigator.languages 协商 > 作品默认语言。浏览器报告的是偏好，不是 IP 位置；不要根据用户所在国家或界面语言推断配音偏好。[R2]

规范化标签、补全可能的 script、资源匹配、格式化数据回退是不同步骤。不能声称 `zh-CN` 的规范化操作必然等于 `zh-Hans`；作品可以在经验证的协商规则中把前者匹配到后者。

推荐保留 script 的显式回退图：例如允许 `zh-Hant-HK -> zh-Hant`，但不自动 `zh-Hant -> zh-Hans`。简繁转换不是作品翻译的替代品。bare `zh` 的匹配必须由已固定的策略和默认项解决，不靠 HashMap 顺序。

本地化资源按显式 URL/对象身份请求，不让同一正文 URL 随 Accept-Language 返回不同内容。这样避免语言变体与缓存键混淆；服务端确实协商时必须相应配置 Vary。[R12]

### 2.2 四种回退

| 类型 | 建议规则 |
| --- | --- |
| UI 消息 | 允许逐消息回退，但整条消息用实际来源语言的 bundle 格式化；保留错误诊断 |
| 剧情文本 | 正式声明支持的章节必须完整；不完整章节用明确的整章节回退策略，玩家事先可见 |
| 配音 | 与文本独立；按作品策略使用指定原声或无可选配音，不随机跨演员/版本拼接 |
| 字体 | 按语言、script 与塑形上下文选择，不能把任何有这个码位的字体都视为正确 |

CLDR/ICU 的数据回退解决格式化与语言数据缺失，不自动决定作品正文应该读哪一种翻译。

## 3. 文案模型：UI 与剧情分离

### 3.1 UI 消息

建议采用 Fluent，Rust 层使用 fluent-bundle，限定可调用格式化函数。FTL 支持变量、select 和 CLDR 复数类别，不需要拼接单词来产生完整句子。[R3][R4]

```ftl
save-count = { $count ->
    [one] One save
   *[other] { $count } saves
}
```

为每条消息建立参数名与类型契约。禁止 UI 消息读取任意 VM 全局变量。解析在加载时完成，格式化仅在依赖变化时发生；输出是呈现文本，不作为下一段模板再解析。

FluentBundle 的 locales 列表用于格式化器协商，不代表它自动拥有其他语言的翻译内容。Resolver 必须显式寻找消息，并使用命中语言的 Bundle 处理复数与格式化。不要把 fallback 英文条目硬塞进中文 Bundle 后用中文复数规则处理。[R4]

动态插值保留双向隔离，不为了「字符串看起来一样」移除隔离控制。由允许名单和类型检查限定外部格式化功能；不能默认所有 Fluent JS 实现的函数都在所用 Rust 配置里存在。

### 3.2 剧情正文

沿用 NIR 的 TextDoc，而不是把所有对白转换成 FTL 中嵌套的可执行标签。

```text
TextContract
  TextId
  MeaningRevision
  Params（名字与类型）
  Gates（身份、出现次数、语义顺序）
  允许的样式、Ruby、插值与页面结构

LocalizedText
  TextId
  locale
  source_revision
  contract_digest
  spans / Ruby / 语义 Marker / 局部呈现参数
```

TextId 标识同一个逻辑文本，文本对象 digest 标识这一份具体字节。一次错字修订可以改变正文对象而保留 TextId；是否保持 MeaningRevision 必须经过内容评审。选择含义改变、Gate 改变、可观察剧情结构变化不是普通翻译修订。

Gate 身份和程序要求的顺序在翻译中保持一致；文本段可在各 Gate 区间内调整语序。只有集合相同但顺序颠倒并不足够。需要改变剧情执行次序的本地化版本应成为受验证的内容变体，而不是偷偷绕过契约。

选项保持 OptionId。翻译替换标题和说明，不能增加或删除可选分支，更不能用显示下标返回结果。

显式剧情分页与视口排版导致的分页分开。长译文可以重排；不能为了塞进同样行数而改变程序等待条件。

### 3.3 配音与演出同步

VoiceKey / TextId 映射到对应语言的 take、资源对象、时长/标记描述及允许的替代。身份不能只依赖顺序文件名。

字幕长度、阅读时间与音频时间不能等同。逐字进度不能按「原语音播放到 60%」直接换算成另一语种字数；需要语义 Marker，或仅在语句完成后切换。

区分静音与不加载配音。静音只影响输出增益，不自动解除剧情 Await。完全不加载可选对白语音，只能用于明确允许无语音的呈现契约；存在必需音频标记或硬等待时，必须保留其时序，或有内容声明的等价替代。V1 不自动改写这些语义。

## 4. Rust 国际化与文字实现

推荐分工：

| 层 | 起点 |
| --- | --- |
| Locale 解析、语言数据、数字/日期与复数辅助 | 按功能选用 ICU4X |
| 播放器 UI 消息 | fluent-bundle 与显式参数契约 |
| 剧情文本 | NIR TextDoc + LocalizationResolver |
| 塑形、字形、布局 | 既有 Rust text 栈，补齐明确的语言能力 |
| 浏览器宿主 | 输入法、语言偏好、必要可访问语义桥 |

ICU4X 支持静态 baked 数据与运行时 BlobDataProvider，也支持按所需组件和 locales 裁剪数据。Blob provider 的格式化数据回退需要单独配置；不可把任意 blob 当成全语言数据。[R5]

P0 优先使用经过裁剪的有限语言数据集，不带所有日历、全部分段模型和所有区域数据。后续有大量语言再评估外置数据；分包不能导致每个包重复一份相同公共表。记录语言算法、数据、分段、塑形与字体版本，以便定位更新后布局变化。

Fluent 自己的语言/复数依赖不自动与 ICU4X 共享。先固定适配配置并检查重复体积，不为了统一名字自行重写复数系统。

### 4.1 排版

逻辑文本 -> 参数解析与方向隔离 -> script/language/bidi 分析 -> 字体与塑形 -> 断行与最终行内重排 -> 字形缓存 -> wgpu。

这不是承诺所有排版实现都是一次单向扫描。实际可能需要按行重新塑形或测量。UAX #9 定义双向文本算法，UAX #14 定义可断行位置而非最终宽度拟合；右对齐不能替代 bidi，字符覆盖不能替代塑形。[R6][R7][R8]

P0 正式语言仅声明通过全链路测试的范围，建议用简中、繁中、日文、英文建立基线。RTL、泰文、复杂 Indic 或竖排按能力分别验收，不把结构支持误报为全部语言已经能发行。

### 4.2 字体

字体计划按文本语言选择优先 face，同样 Han 码位在不同语言需要核对字形风格和 locl 行为。Fallback 应保留组合簇和 script 上下文，不能按单个字符拆散连写。

对子集保留 GSUB/GPOS、mark、mkmk、rlig、locl 等需要的 OpenType 规则及其字形闭包。fontTools 的 subset 支持基于字符/字形和布局规则的子集；禁用 layout closure 或粗暴删表可能破坏塑形。[R9]

基础 UI 字体、语言公共字体、章节增量字体与动态名字 fallback 独立规划。只扫描静态对白的字符集合不足以覆盖玩家名字、选择变体、数字日期、错误提示与备用语言。系统字体在浏览器里的可用性不能成为 Rust 字体计划的隐式保证。

子集更换会改变 face 内容和 glyph ID，缓存键必须包括字体 digest，而非仅「某某字体」显示名。WOFF2 的浏览器 CSS 使用路径不等于 Rust 可以直接读同一数据；仅选用文本栈实际支持或有显式解压适配的格式。

### 4.3 UI 与输入

布局使用 start/end、wrap 与弹性宽度，不按固定字符数设按钮宽度。竖排、RTL 组件重排与整个舞台的镜像是三件事：不能因为语言切换就镜像 CG、地图或角色位置。

日期保存为真实时间戳，时长保存为数值，存档槽保存为 ID，不解析本地化字符串来恢复数据。存档显示采用玩家格式化设置，不随文案回退意外改变时间含义。

Rust 绘制 UI 仍需通过宿主接入输入法 composition 和可访问语义；候选词未提交时不能把 Enter 当成推进剧情。设置页面 lang 与双向语义，按完整消息朗读，不逐字朗读打字机。

## 5. 语言切换协议

### 5.1 安全边界

- UI 语言：当前组件完整准备后，以原子方式更换标签、布局、语义与焦点；不改变动作身份。
- 正文：默认下一次 DialogueOpen 或明确页面边界生效。当前已出现对白不重新执行。
- NVL/追加页面：等到页面清除或明确的重新呈现边界，不能半页混用新旧翻译。
- 语音：下一句或经声明的媒体边界生效，不映射当前播放百分比。
- 选项：opened/offered snapshot 保持；第一版不在玩家选择时更换当前选项文本集合。

菜单显示 requested/resolved 差异，必要时允许取消待应用的设置。

### 5.2 状态机

```text
Request
 -> Resolve（固定 release 内的包）
 -> Validate contracts/capabilities
 -> Reserve joint budget
 -> Prepare text/font/glyph/UI/audio/localized visual deps
 -> PendingReady
 -> Commit at safe boundary
 -> Update epochs and leases
```

切换失败维持旧上下文，不产生半套新字体和旧文本。连续多次切换只提交仍有效的请求；旧请求可以贡献经校验的共享字节缓存，但不能修改当前 resolved locale。

LanguageSelectionId 由解析后的 UI、正文、语音、格式化配置及其包 digest 派生。NIR-0002 的 ReadyLease 新增它或等价分项身份；不能只检查 typography_epoch，因为仅语音或文本修订也可能过期。

提交时作废相关 Near/Speculative 需求，不清空语言无关 CG/BGM 缓存。资源是按真实内容 hash 共用，不以每种语言复制一套。

### 5.3 存档、已读与回看

快照记录 active selection、实际 resolved locale、文本对象/语音对象身份、冻结插值和当前 Gate。UI/玩家偏好仍独立，不因读档被全部回滚。

恢复时优先精确恢复当前实例，之后在下一个安全边界应用玩家偏好。旧包被移除时只能使用经验证迁移或指定检查点重启；不能套用新文本的同一字符偏移。

已读默认按 TextId + MeaningRevision 管理，可扩展按语种分别记录的阅读偏好。原文与含义重要修订不得仅靠 ID 未变就强制跳过。这个策略要在配置与测试中明确。

历史记录保存当时解析的文本或内容引用加冻结参数；「查看其他译文」是另一个显示模式，不改变已发生记录，也不触发语音/剧情任务。

## 6. 分包：以共享与加载生命周期为单位

建议包类型：

```text
runtime/                固定的 WASM、JS glue、Worker、基础 shader
program/                语言无关 IR，按章节或模块
shared-assets/          背景、立绘、BGM 等公共对象
ui/<locale>/            播放器消息
text/<locale>/<module>/ 正文、说话人、选择和 Marker 位置
voice/<locale>/<module>/语音索引与媒体对象
localized-visuals/      含文字图片/特定语言演出资源
fonts/                  字体与字体计划
locale-data/            按需的语言数据
```

包首先是一个 manifest 与对象集合，不必是连续归档。正文不能每句话一个 HTTP 请求；可按稳定章节/场景簇分块。大型图片、音频保持独立或小的独立可寻址段，避免修改一句话让整个大包失去缓存。

首屏集合 = 引擎 + 当前必要 UI + 当前/恢复章节 IR + 选中文本 + 字体 + 必要舞台/音频。开启第二语言并不复制 WASM 和所有 CG；仅根据依赖差集下载。

计量同时报告引擎字节、首句闭包、当前语言全文、配音增量、全部可选内容、某次更新差集。磁盘文件大小、HTTP 实际传输量、解码后内存分别统计。

## 7. 发行版本与不可变清单

### 7.1 身份层次

| 身份 | 用途 |
| --- | --- |
| GameId | 存档与作品命名空间，版本升级不变 |
| IR semantic version + capabilities | 解释执行所需语义 |
| Engine ABI/build digest | JS glue、Worker、WASM 和宿主协议配套 |
| Program / content revision | 剧情控制流与稳定节点 |
| Text / voice / font / locale data digest | 具体呈现内容 |
| Save schema | 持久化数据可读写范围 |
| ReleaseManifest digest | 发布者验证过的一组精确组合 |

SemVer 区间可以表达兼容候选，但最终会话必须选择一个精确已验证的对象图。单纯「engine >= 1.0」不能替代配套的 JS/WASM 和输入协议验证。

### 7.2 建议目录

```text
/game/
  index.html                     小型稳定启动/错误壳
  bootstrap.js                   小型、保持协议兼容的加载器
  channels/stable.json           可变入口，只指向一个 release
  releases/<manifest-digest>.json
  objects/<digest>.<ext>          不可变内容
  app.webmanifest                可选 PWA，稳定应用身份
  sw.js                          可选，版本独立于游戏内容
```

ReleaseManifest 指向包清单；包清单指向对象和受控依赖。允许按需取尚未缓存的对象，但其 digest 从启动起已被固定。不含自指 hash 字段：先序列化 release 文件，再由外部 channel 指向其 digest。

引擎的 glue、WASM、Worker 等必须作为配套组合选取。JS 自带相对导入与 WASM 路径需要最终重写、打包或由宿主显式传入；不能最后给每个文件随意改哈希名而不更新引用。SCC/循环 import 可由 bundler 或稳定构建目录处理，不做朴素递归自哈希。

### 7.3 哈希范围与可信边界

推荐 SHA-256 作用于最终产物的 identity 字节：即 HTTP Content-Encoding 解压后的文件内容，不是图片解码像素。媒体本身的编码仍属于文件字节。Brotli/gzip 是传输表示，可以共享同一逻辑对象身份；gzip 元数据等也要固定，才有传输文件的重复构建一致性。

哈希后不再改写文件。字符编码、换行、资源转换、JS 路径处理、wasm-opt、strip 等全部在最终 hash 之前。

ETag 是 HTTP 验证器，不保证是内容 SHA；manifest 明确记录 digest，不拿任意 CDN ETag 代替它。[R12]

哈希检查不证明发布者身份；SRI 可校验适用的浏览器子资源，但不会自动保护所有任意资源请求或防止可信启动页被替换。[R19] 第三方内容或镜像需求出现时再加入明确的签名/信任根设计。V1 同源 HTTPS、不可变 URL、依赖校验与 CSP 是基础。

全文件校验与流式音频/流式 WASM 有缓冲成本，不能声称给每个请求算完 hash 后仍完全零等待。小 IR/清单/文本先验证再解析；大资源按已声明策略采用分块验证或受信传输，不无界复制几份完整缓冲。

## 8. 可重复构建流水线

```text
固定源码、Cargo.lock、Rust toolchain、构建工具与数据快照
 -> 验证 NIR 与语言契约
 -> 生成正文/消息数据、字体与媒体派生资源
 -> cargo build --locked --release --target wasm32-unknown-unknown
 -> 匹配版本的 wasm-bindgen --target web
 -> 可选的固定 Binaryen 优化与最终 WASM 验证
 -> 处理 JS/Worker/资源路径和 release profile
 -> 最终字节哈希、生成 manifests
 -> 生成传输压缩与大小报告
 -> 对最终产物浏览器测试
 -> 部署 staging、公开 URL 验证、晋级入口
```

wasm-bindgen 的 web 输出可以作为 ES module 直接加载；不需要为了静态播放器先引入一个大型前端框架。复杂宿主依赖确实需要 bundler 时，再将其纳入固定构建链。[R10]

Cargo.lock 与 --locked 固定依赖解析，不自动保证跨环境 bit-for-bit。还要固定字体与 ICU 数据生成器、媒体编码器、Binaryen、构建容器、时间戳、输入排序、随机种子和压缩参数，并通过独立重复构建比较最终 digest。[R11][R16]

### 8.1 Release 配置

起点可为 opt-level=3、LTO、单 codegen unit、panic=abort、去除 release 调试信息。然后对 s/z 与不同 LTO 策略测体积、冷启动和布局/解码吞吐，不默认最小包必然体验最好。Cargo 明确支持这些 profile 选项，但优化等级收益需要测量。[R11]

不可把可恢复的内容错误变成 panic；用 Result 和结构化错误路径。私下保存与实际 build digest 对应的调试信息/源码映射，不把原始资源和整个调试工具链带到公开包。

Rust/LLVM 的默认 WebAssembly 特性会随版本演进，依赖也可能单独启用特性。因此要记录并验证最终 WASM 使用的指令能力，不能仅凭 WebGL2 后端就宣称支持任意旧浏览器。[R17]

### 8.2 发布档位

P0 维持一个无共享内存、WebGPU 主路径＋WebGL2 兼容能力的标准包。只有真实包体数据证明必要时再分后端产物。

可选 isolated 档位需要独立部署能力测试，不能自动把普通 Worker 当成共享 WASM 线程。多语言一般是数据选择，不是每种语言各编译 WASM。

## 9. 托管契约

发布的是可通过 HTTP(S) 部署的目录；提供 ZIP 时注明需静态服务器，不承诺 file:// 双击运行。测试根目录、子目录和 iframe 宿主三种情形，路径都由 base URL/manifest URL 解析，不到处写死 `/assets`。

### 9.1 内容类型与压缩

WASM 用 application/wasm；JS 用有效 JavaScript MIME；JSON、媒体与字体用对应类型。instantiateStreaming 正常路径要求正确的 WASM MIME。[R13]

通过内容协商提供压缩时：请求仍指向逻辑 .wasm URL，响应 Content-Type 为 application/wasm，Content-Encoding 与实际 br/gzip 一致，Vary 至少正确区分 Accept-Encoding。不要直接把一个 .br 文件按不带 Content-Encoding 的普通二进制发送。[R14]

大多数已压缩图片、音频是否再压缩应测收益；不要强制把整游戏包压两遍。需要 Range 的媒体单独验证状态码、Content-Range、编码和缓存路径。

资源 404 返回 404，不能被 SPA 回退伪装成 200 index.html。错误响应不得带一年的 immutable 缓存。

### 9.2 缓存策略

| 对象 | 建议 |
| --- | --- |
| index/bootstrap/channel | no-cache + 验证器 |
| 哈希 release/包清单/对象 | public, max-age=31536000, immutable |
| Service Worker 脚本 | 保持更新检查，不长期 immutable |
| 私人存档导出 | 本地文件或明确的私有存储，不放公共 CDN |

no-cache 不是禁止存储，而是复用前验证。仅入口更新时不需要清空所有内容缓存。[R12]

### 9.3 安全与跨源

基础档优先同源：简化 Worker、音频、下载校验和缓存。跨源资源需要正确 CORS；opaque no-cors 响应不能被当成已验证字节。

CSP 使用窄化权限。允许 WASM 编译时优先 wasm-unsafe-eval 而非给所有 JS unsafe-eval；Worker 的策略与其加载路径也需要测试。[R18]

共享内存增强档通常需要 COOP/COEP，以及所有资源满足相应跨源隔离要求。不要在基础档无条件打开后再发现第三方资源或嵌入环境被阻断。[R20]

公开版本必须包含依赖与资源来源/许可清单，构建系统检查遗漏；开发密钥和其他秘密不能放入浏览器包。资源混淆不是 DRM 或授权控制。

## 10. 发布、更新与回滚

### 10.1 原子发布是入口原子，不是整个 CDN 同时更新

```text
构建 Release B
 -> 上传全部新 hash 对象，不覆盖 A
 -> 上传并验证 B 的包清单与根清单
 -> 从实际公开路径 GET 校验关键对象、MIME、压缩与 CORS
 -> 标记 B 可发布
 -> 最后切换 stable 指针
```

不同边缘节点可能暂时给 A 或 B，这不影响正确性，前提是 A/B 都是完整不可变图。失败时不晋级；并发发布使用托管系统支持的条件写或串行晋级，不能后提交的旧任务覆盖新入口。

客户端读入口一次得到候选 release，验证兼容性后固定 SessionRelease。游戏中发现新发行版只提示，不替换 WASM、正文或字体。玩家同意并保存后，在重启/明确切换流程使用新图。

### 10.2 回滚

指针回到 A 只影响之后的启动，不自动撤销 B 的存档写入或数据库结构升级。B 存档若 A 不支持，应保留并提示兼容版本，不伪装能读。

保守保留旧 release、清单和其共享对象，满足已承诺的存档支持期限。内容寻址可去重，但「只保留最近两个版本」未必足够。删除前考虑离线副本、休眠标签页与旧存档；没有心跳不能证明无人使用。

### 10.3 数据库

GameId 是命名空间，发布目录名不是。缓存与存档分别管理，release 变化不要每次都升级 IndexedDB 的整个数据库版本。

必要升级使用新增字段/对象与明确迁移，处理 versionchange 和其他标签页阻塞；迁移前保留备份，不删除原始快照。[R23] 旧引擎不支持新存档时只读或拒绝；发布回滚不等于数据库回滚。

浏览器存储按 origin 管理，同源多作品共享配额，换域名后旧存档不会自动出现。保留导入导出和持久化申请，不能承诺浏览器一定永久保存。[R22]

## 11. 离线与 PWA

P0 在线静态版先完成正确更新与恢复。P1 增加显式的「离线保存本章/本语言/该配音」和 PWA；不强制第一次访问下载所有章节、字体与配音。

OfflineSelection：

```text
release_digest
module_set
ui_locale / text_locale / voice_locale
quality/backend fallback requirements
object_closure
verified_completion_state
```

离线闭包必须包含该版本的启动 runtime、UI、IR、选中文本、字体、必要媒体以及保证的渲染替代。只缓存文字不等于章节可离线。所需模块中的全部可访问分支需覆盖；动态引用依赖不能无界或漏报。字幕模式、原声模式、高清模式分别显示已验证可离线状态。

CacheStorage 与 IndexedDB 不构成跨 API 的原子事务。先在 staging 缓存写入并校验，最后提交「完整闭包」索引；崩溃恢复时重新校验，不因索引存在就相信缓存没有被清理。[R21][R22]

Service Worker 提供导航和选中对象的离线访问，不在 install 中缓存整个游戏；不无条件 skipWaiting + claim + 删除旧缓存。生命周期允许新 Worker 等旧页面释放，强制接管会改变这个过程。[R15]

离线入口明确回到本机已完整准备的 release，不能拉一个最新 runtime 再尝试旧内容。PWA 的应用身份保持稳定，不把 release digest 编入 manifest id；发布修订不应表现为另一款已安装应用。[R24]

## 12. 验证与发行门槛

### 国际化

- BCP 47 有效性、协商与回退无环；必需 fallback 依赖存在。
- 正式支持的 UI/message、正文与 OptionId 覆盖完整；source revision/contract digest 匹配。
- 参数类型、Gate 次数与语义顺序一致；没有翻译中隐藏执行命令。
- 字体子集覆盖静态内容与动态场景，实际塑形无缺字；日/中相同码位字形、Ruby、大字号、长选项和禁则测试。
- 伪本地化扩长、RTL 混排、组合附加符与长名字；未正式支持的语言不伪报通过。
- 切换连续点击、断网、准备失败、NVL/选项/语音中切换、旧 callback、回看和读档。
- 相同逻辑输入下语种改变不改变变量、分支与选择；允许文本/语音持续时间不同。

### 发布

- 最终 URL 子路径正确，WASM/JS MIME、Content-Encoding、CORS/CSP、错误状态正确。
- 重复构建对象一致；hash 和字节数正确；JS/WASM/Worker 配套，最终 WASM feature profile 可运行。
- A 会话运行中发布 B、读取新旧存档、多个标签页、回滚入口、语言包只更新一项均不混版本。
- 旧文件仍可按其 hash 访问；404 不变成 HTML；损坏对象不被标成离线完成。
- PWA/离线若启用：缓存满、缓存被逐出、离线启动、缺配音与中途下载取消；恢复旧版本不清除存档。
- 模拟浏览器测试加实际目标设备验证；未运行的配置必须显示未测量。

## 13. 实现顺序

1. TextContract/LocaleContext、UI 与正文包，有限正式语言集；保持语音独立。
2. LanguageSelection + ReadyLease + 安全边界切换 + 快照/历史。
3. 内容寻址 ReleaseManifest + 精确依赖组合 + 静态目录产物。
4. 固定构建工具链、最终产物测试、staging 晋级、保守旧版本保留。
5. 离线闭包、受控 Service Worker/PWA；有测量需求时再分 WASM 档位和外置更多语言数据。

建议新增模块：nir-locale、nir-messages、nir-localization、nir-package、nir-release-check。只有宿主识别 HTTP/IndexedDB/Service Worker；内容和运行层使用固定的逻辑身份及错误类型。

## 参考资料

核对日期：2026-09-20。链接用于支持标准/库行为；具体接口实现应固定版本，不把 latest 当永久协议。

[R1] W3C Language tags： https://www.w3.org/International/articles/language-tags/
[R2] MDN Navigator.languages： https://developer.mozilla.org/en-US/docs/Web/API/Navigator/languages
[R3] Fluent selectors： https://projectfluent.org/fluent/guide/selectors.html
[R4] fluent-bundle： https://docs.rs/fluent-bundle/latest/fluent_bundle/bundle/struct.FluentBundle.html
[R5] ICU4X data management： https://icu4x.unicode.org/2_2/tutorials/data-management/
[R6] Unicode UAX #9： https://www.unicode.org/reports/tr9/
[R7] Unicode UAX #14： https://www.unicode.org/reports/tr14/
[R8] HarfBuzz shaping concepts： https://harfbuzz.github.io/shaping-concepts.html
[R9] fontTools subset： https://fonttools.readthedocs.io/en/latest/subset/index.html
[R10] wasm-bindgen deployment： https://wasm-bindgen.github.io/wasm-bindgen/reference/deployment.html
[R11] Cargo profiles： https://doc.rust-lang.org/cargo/reference/profiles.html
[R12] MDN HTTP caching： https://developer.mozilla.org/en-US/docs/Web/HTTP/Guides/Caching
[R13] MDN instantiateStreaming： https://developer.mozilla.org/en-US/docs/WebAssembly/Reference/JavaScript_interface/instantiateStreaming_static
[R14] MDN Content-Encoding： https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Content-Encoding
[R15] MDN Service Worker lifecycle： https://developer.mozilla.org/en-US/docs/Web/API/Service_Worker_API/Using_Service_Workers
[R16] Cargo build --locked： https://doc.rust-lang.org/cargo/commands/cargo-build.html
[R17] rustc wasm32-unknown-unknown： https://doc.rust-lang.org/rustc/platform-support/wasm32-unknown-unknown.html
[R18] MDN CSP script-src： https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Content-Security-Policy/script-src
[R19] MDN Subresource Integrity： https://developer.mozilla.org/en-US/docs/Web/Security/Defenses/Subresource_Integrity
[R20] MDN Cross-Origin-Embedder-Policy： https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Cross-Origin-Embedder-Policy
[R21] MDN Cache： https://developer.mozilla.org/en-US/docs/Web/API/Cache
[R22] MDN storage quotas and eviction： https://developer.mozilla.org/en-US/docs/Web/API/Storage_API/Storage_quotas_and_eviction_criteria
[R23] MDN IndexedDB versionchange： https://developer.mozilla.org/en-US/docs/Web/API/IDBDatabase/versionchange_event
[R24] MDN Web app manifest id： https://developer.mozilla.org/en-US/docs/Web/Progressive_web_apps/Manifest/Reference/id
