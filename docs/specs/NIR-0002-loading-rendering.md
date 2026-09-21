# NIR-0002：Web 首版的加载与渲染架构

状态：设计提案 v0.1。日期：2026-09-20。

本提案承接 NIR-0001，不涉及旧引擎内容转换或作者工具。剧情、场景、时间轴、文字布局和游戏可视 UI 使用 Rust/WASM；GPU 渲染使用 wgpu。WebHost 负责浏览器输入、下载、媒体解码、音频输出、持久化和必要的语义辅助。本文中的预算、接口和验收门槛都是待实测的设计起点，不是现有播放器的测试成绩。

## 1. 设计结论

加载系统不是一组 fetch，渲染器也不是每帧遍历全部事件。两者共同实现一个“演出准备系统”：

```
NIR 当前执行状态 / 控制流资源摘要
             |
             v
       DemandPlanner
             |
             v
       PrepareCoordinator
       /       |         \
   资源 DAG  内存准入  管线 / 文字准备
       \       |         /
             v
          ReadyLease
             |
             v
       Activate 原子提交
             |
             v
   场景变化 / 时间轴采样 / UI 投影
             |
             v
      增量 Prepare + 绘制计划
             |
             v
          wgpu Queue
```

“下载完成”“已经发出上传”“可供有序绘制使用”“GPU 已经执行完毕”“用户看见画面”分别计量，不能使用一个 ready 布尔值混淆。

默认优化顺序：正确性与恢复 > 首句可阅读 > 阅读中的稳定帧时间 > 清晰度与低功耗 > 平均吞吐量。任何降低质量的策略不得改变剧情结果、图层顺序、交互契约和必需演出。

## 2. 浏览器与执行宿主

### 2.1 后端能力

主测试路径为 wgpu/WebGPU；保留 wgpu/WebGL2 的基础渲染路径。查询实际适配器、设备和 surface 能力，不用浏览器名字推断可用特性。[R1][R2]

基础画面使用 textured quads、实例化、uniform、采样纹理、片元 shader 和有限合成 pass，不依赖 compute、storage buffer、bindless 或某种 GPU 压缩格式。WebGL2 降级不是一套不同的剧情引擎。wgpu 的 downlevel limits 明确有 storage/compute 等限制，申请 limits 时要与实际资源尺寸匹配。[R3]

首个工程构建可以包含两个后端，便于 A/B；确认兼容正确后再评估分为 WebGPU 主产物和按需获取的兼容产物。两者共享 IR 和语义，不能同时无条件下载。wgpu 的 feature 与后端依赖关系可作为构建裁剪依据，但真实体积必须测量。[R1]

### 2.2 推荐线程布局

```
浏览器主线程：输入、页面生命周期、音频用户手势解锁、DOM 语义桥、启动与恢复壳
Runtime Worker：Rust VM、场景、文字、UI、wgpu、OffscreenCanvas
Asset Worker（初始一个）：重解码、转码、必要资源预处理
```

先做 capability smoke test。能够转移 OffscreenCanvas、创建所需图形上下文并驱动帧循环的组合才启用 Worker 渲染；否则，同一 Runtime 在主线程运行，重解码依然外移。OffscreenCanvas 与 Worker rAF 有浏览器接口，但应验证完整库组合，不把单个 API 存在当成整个栈可运行。[R4]

基本路径不依赖共享内存。普通 Worker 与可转移对象不等于共享 WASM 线程；后者需要跨源隔离条件。没有 SharedArrayBuffer 时，也能让独立 Worker 通过消息工作。[R5][R6]

GPU 对象只属于一个 owner。不要传 GPU 纹理、wgpu 句柄或 Rust 堆指针给另一个 Worker；传标识、明确拥有权的字节缓冲或 ImageBitmap。转移 ArrayBuffer 避免一次跨线程复制，不证明从网络到 GPU 整条链路零复制。[R1][R6]

将 VM 与渲染器放在同一 owner，避免每帧序列化整个 SceneState。按钮/确认输入立即发出；仅合并可替代的 pointer-move，不能合并丢弃按下、松开和选择。

## 3. 首屏关键路径

### 3.1 最小入口

```
小型 HTML / CSS / Host
 ├─ 流式加载和编译引擎 WASM
 ├─ 读取 bootstrap manifest
 ├─ 读取本地设置和可继续入口
 └─ 请求确定需要的首屏资源
        |
WASM 实例化 -> 初始化图形 -> 准备基础管线
        |
入口资源、字体、UI、音频就绪 -> 首句可交互
```

不先解压全游戏，不下载所有章节清单，不解析完整脚本，不初始化未使用的视频/角色动画 SDK，不预热所有 shader。

只有收到可继续入口后才把它作为首屏目标；不能为了“继续阅读”先无条件下载第一章。

WASM 使用流式实例化路径，响应必须具有正确的 application/wasm 类型；HTTP 内容编码由浏览器处理。保留非流式兼容路径，但不要在正常路径先把整个 WASM 转为 ArrayBuffer 再编译。[R7]

拆开三个指标：壳可见、标题可交互、首句可阅读。占位海报不算 Rust/wgpu 首帧。开始阅读的真实用户操作中执行音频恢复；必要时单独提供“启用声音”，不能让自动播放限制伪装成无限加载。[R8]

### 3.2 内容组织

小型 bootstrap 指向章节 manifest；章节 manifest 指向 IR 模块和内容哈希资源。首版使用独立资源文件，避免巨大压缩包和对 Range 的强制依赖。大量很小的文件确实成为瓶颈后，再按共用生命周期合并独立可寻址块。

资源的逻辑 ID 与变体 URI 分离；模块缺失在明确边界准备。首章不必验证尚未下载的全部模块，但已加载模块及跨模块声明必须被验证。

## 4. 需求计划与多阶段调度

### 4.1 需求类型

```
Required：当前 Activate 必需
Near：下一确定 Cue / 下一对白
Speculative：选择后的候选分支
Background：离线章节、可选高清等
```

当前必需需求压过预测需求；读档、新选项与跳转立即重算需求。当前场景和转场两侧保持引用。远期资源可以只保留压缩数据，不必解码，更不必驻留 GPU。

预测读取 IR 的资源摘要与控制流，不执行会更新变量、随机状态、任务或持久数据的剧情。纯条件可结合当前已冻结变量限制候选；不确定时停在边界或做受预算约束的并集。循环有节点和字节上限。

可作为初始策略的范围：下一 1—2 个明确 Cue、接下来 2—3 句的字体/语音需求；数值随实际阅读方式与设备修正。选择未定时优先共用资源；不同分支默认仅预取压缩数据。用户快读时不能保证有足够预取时间，因此任何正确性都不能依赖预测命中。

### 4.2 优先级

同一级别内根据到期时间和预计准备成本调度：

```
slack = needed_by - (now + remaining_prepare_p95)
```

remaining_prepare_p95 包括排队、下载、解码/转码、上传和首次使用准备的剩余关键路径。历史时延用于预测，不影响剧情分支。

分别限制网络并发、CPU 重任务数、解码工作集、GPU 上传份额和总驻留。异步语法不让一次同步大图解码自动移到另一线程。后台需求不允许挤满所有解码槽；正在使用的媒体下载拥有必要带宽保留。

### 4.3 缓存状态不是一个单调枚举

一个资源可以同时“磁盘压缩数据仍在”“CPU 解码数据已丢弃”“GPU 变体仍驻留”。分别管理每层表示：

- 内容字节：hash / 状态 / 校验 / 获取请求。
- 解码表示：像素或 PCM、内存估算、颜色/alpha 语义。
- GPU 表示：格式、mip、设备代次、上传序号、引用与使用状态。
- 派生表示：布局、字形、合成面、管线。

通常流程是缺失 -> 获取 -> 解码 -> 上传 -> 可使用，但淘汰只影响某些层，不应退回一个错误的全局状态。

## 5. 准备屏障、预算准入与 ReadyLease

### 5.1 准备集合

一个 Cue 的准备集合除了图片和声音，还包括字体/字形、文本布局、纹理采样状态、目标管线、转场遮罩、离屏面和 UI 组件依赖。

准备流程：

```
冻结操作数
-> 派生完整必要资源 DAG
-> 计算去重后的联合峰值
-> 做预算准入和 pin 预留
-> 准备资源与绘制路径
-> 获得 ReadyLease
-> 检查代次与呈现配置
-> 在帧边界提交逻辑状态
```

预算不够时优先取消预测、释放冷资源，然后选择已批准的较小画质变体；仍不满足时保留当前画面并返回可解释的失败。不能让多个部分准备任务各自固定半套资源，最后互相等待永远凑不齐一整个 Cue。

### 5.2 资源租约

ReadyLease 至少关联：Cue 实例、冻结操作数身份、内容版本、session/device/surface/typography 代次、变体选择、强引用集合、准备好的场景计划，以及上传依赖。

它不是剧情 IR 的 GPU 句柄，而是宿主内部的“仍可使用”的保证。租约在激活时转交给场景/任务作用域；过期时重新准备派生状态，不能重新执行剧情赋值。

防止三类竞争：就绪后纹理被 LRU 淘汰；就绪后窗口/DPR/字号改变导致布局失效；旧设备回调修改新设备记录。

### 5.3 GPU 就绪语义

GPU 同队列的正确顺序可以保证后续绘制在前序上传之后使用资源，不必每帧等待全队列完成。对于首用预热或特别关键的大准备批次，可以异步观察一次完成；不能阻塞事件循环，也不能把所有绘制串行化到 fence。[R9][R10]

保留独立的 UploadEnqueued、OrderedUseReady 与 WarmupCompleted 观测状态。Queue.submit 返回不是“玩家看见”；完成回调也不是屏幕扫描证明。

NIR-0001 的默认语义保持：必要准备期间冻结 Story 和耦合媒体，UI 时间继续。预取提高命中率来减少这种停顿；环境 BGM 独立继续需要新的显式时间策略，不在本提案中暗改。

## 6. 图片与字体资源的技术选择

### 6.1 先分传输与 GPU 格式

PNG/WebP/AVIF 等是传输/解码形式；常规解码后上传 RGBA8，不会因为下载文件小而自动获得更小的 GPU 纹理。KTX2 是容器，Basis Universal 可提供 GPU 格式转码路径，须检查实际设备支持。[R11][R12][R13]

建议：基础路径 PNG/WebP；AVIF 只在解码实测和画质收益明确时选择；KTX2/Basis 作为大资源优化路径。正文不做图片，UI 小图与字形走单独图集。

| 资源 | 建议的起点 |
| --- | --- |
| 背景/CG | 多分辨率浏览器图片；评估 KTX2 GPU 压缩变体 |
| 透明立绘 | 保真 alpha 的浏览器图片；另测 UASTC 品质和上传收益 |
| 小型 UI 图标 | 小图集/九宫格，不与大 CG 混在一个巨图集 |
| 遮罩 | 不作为色彩图处理；优先单通道语义 |
| 字形 | R8 覆盖率图集，彩色字形独立处理 |

不能用“ETC1S 一定更小更好”或“UASTC 一定更快”替代目标内容的测试。转码器下载、初始化、工作集和转码时间必须计入首屏与峰值。无适合 GPU 压缩格式时，优先选择已验证的普通图片变体，而不是必然付费转码后再膨胀为 RGBA。

### 6.2 两条准备路径

浏览器图片路径：Fetch/Blob -> createImageBitmap -> 外部图像上传适配 -> 规范化纹理。

受控字节路径：受限解码器或 KTX 转码器 -> 明确格式的数据 -> wgpu 上传。

createImageBitmap 支持 alpha、颜色转换、方向和缩放选项。WebGPU 的外部图像复制允许浏览器选择复制方式，但不保证零复制、不保证无阻塞；接到所锁 wgpu 版本的适配必须测试。没有对应快速路径时使用受预算的像素上传。[R14][R15]

不要把 Blob、ArrayBuffer、Rust Vec、ImageData、ImageBitmap 和上传 staging 的完整副本全部同时保留。浏览器图片句柄完成交接后释放；资源需要可恢复来源，但无需永远保留解码后的 CPU 大图。

### 6.3 尺寸、mip 与图集

根据实际显示尺寸、最大镜头放大与玩家画质设置选择资源，不仅看设备 DPR。缩小显示或镜头缩放明显的资源准备 mip；从不缩小的小 UI 不必无条件带完整 mip 链。

二维完整 mip 链的面积接近基础级的 4/3；精确成本根据每级尺寸和块压缩对齐计算。离线生成 mip，避免首用时集中生成。[R13]

图集按生命周期分组、保留边缘 padding，并按设备尺寸 limits 分页。大背景和大立绘通常保持独立纹理；不要为少几个 draw call 把全章绑定在同一个巨图集中。可保留原本已分层的身体/表情小图，但不要求运行时自动推导差分。

## 7. 内存预算与峰值控制

RGBA8 基础纹理成本为宽 × 高 × 4。示例：

- 1920×1080：7.91 MiB。
- 3840×2160：31.64 MiB。
- 三张 4K RGBA8 离屏面：94.92 MiB。
- 2048×2048 的 R8 图集：4 MiB。

这些只是像素存储推算，不包括驱动对齐、浏览器合成、其他图像、mip、临时资源和系统开销。

转场峰值必须统计当前源资源、新目标资源、离屏面、字形、上传 staging、解码工作集与同时存在的音频。内存准入按联合峰值，不按单个资源最终大小。

起始参考策略（不是设备能力保证）：GPU 可追踪资源总预算 192 MiB；其中为转场/临时面预留 32 MiB；CPU 图片解码工作集 48 MiB；PCM 24 MiB；应用自持压缩字节 16 MiB。实际作品不足以放入时，需要重选变体/预算或声明设备门槛，不能偷偷超额分配。

GPU 总预算包含图片、字形、buffer、离屏面等，预留是预算的一部分，不是额外免费内存。WebGL 没有通用可用显存查询接口；应做可追踪估算并保留余量，不宣称估算等于进程总内存。[R13]

淘汰优先级：未选择分支的 GPU 表示 -> 冷解码数据 -> 非活跃已用资源。当前场景、当前交互和转场两侧不可驱逐。快照保存逻辑描述，不固定整个回退历史的 GPU 资源。

释放 Vec 允许分配器复用内存，但不应把它当作 WASM 线性内存立即缩小的保证；峰值控制比在结尾批量 drop 更重要。

## 8. GPU 上传与管线准备

### 8.1 上传预算

所有创建纹理、图集更新与上传由 GPU owner 排队处理。CPU worker 完成解码后只交付待上传工作，不能直接绕过上传预算。

活跃演出时按预计时间与字节双预算安排上传。例如参考档可从每帧 2 MiB 开始实验，但大 API 调用不能靠预算设置自动被抢占。能分块的上传按行/压缩块对齐切片；无法有效分块的操作应更早完成、降低变体尺寸或进入显式准备屏障。

不要在一帧内把所有刚下载完的图片全部 queue.write_texture。该方法具有复制与排队成本，返回并不代表 GPU 执行完成。[R9]

普通渲染提交保持浅队列；GPU 落后时减少预测上传，不增加更多尚未处理的帧。没有可见动画时仍需推动必要上传提交，不能因为停止绘制而让资源准备永远不完成。

### 8.2 小型稳定管线集合

首版管线限定为 sprite、text、简单 UI、mask、有限 transition、最终合成。实例颜色、透明度、UV 和进度是数据，不是重新生成 shader 的理由。

PipelineKey 至少包括 shader 变体、目标格式、sample count、blend、clip 与 vertex layout。基础管线在首屏前准备，章节特有管线提前准备。

浏览器 WebGPU 存在 createRenderPipelineAsync；不要据此假定所用 wgpu Rust 版本直接暴露同名方法。基线可以采用提前创建与少量离屏试绘；桥接异步创建时单独验证资源所有权与版本接口。[R16]

跨会话持久管线缓存不是 Web 首版保证。查询到的 wgpu PipelineCache 文档列出的有效后端只有 Vulkan，不能把它当成浏览器缓存 API。[R17]

## 9. 渲染器：小型 2D 合成，而非通用 3D 框架

### 9.1 热路径

```
输入/任务里程碑
-> VM 有界执行
-> CPU 求值视觉轨道
-> 更新脏变换和脏 UI 状态
-> 已准备资源生成绘制计划
-> 小量 buffer 更新
-> 编码必要 pass
-> submit / present
```

热路径不解码图片，不解析整章，不下载字体，不首次构建复杂管线，不同步回读截图。

缓存分层：逻辑场景 revision、文本布局、字形位图、GPU glyph atlas、固定 UI 几何、管线和可选离屏合成面。不是所有子树都要缓存成纹理：简单几张 sprite 的重绘可能比多占一张全屏缓存和合成更划算。

### 9.2 绘制与合批

使用共享 quad + 实例数据，实例包含 transform/UV/颜色等。只合并相邻且图层语义兼容的绘制，不为了纹理相同跨过其他透明对象重排。

不默认使用 depth、全屏 MSAA 或巨型 render graph。矩形裁切优先 scissor，复杂 clip 才使用 mask/stencil。组整体透明度需要正确离屏隔离时，不能为了性能改成逐子元素透明度。

正常帧尽量直接绘制；只有分辨率分层、缓存、隔离组和特效需要时才增加离屏 pass。

### 9.3 转场

NIR-0001 的冻结源与目标描述允许：源合成一次、目标合成一次，然后每帧只用进度和遮罩合成两张纹理。独立 UI 根继续在上层更新。

源侧含活跃动画时，只能在 Activate 的实际逻辑起点采样；可以提前准备管线和资源，但不能提前冻结源侧时间。激活前的最终快照渲染必须纳入首帧预算。

必要离屏面提前预留；重建 GPU 时从逻辑描述重新生成两侧。转场结束后可按引用和提交生命周期回收，不在每一帧新建/销毁纹理。

## 10. 色彩、alpha 与采样一致性

采用 NIR-0001 的 SDR/sRGB 输入、线性光合成、预乘 alpha 基线。源资源必须标注颜色编码和 alpha 形式；规范化只发生一次。

预乘的定义是在线性颜色上乘 alpha；不要无条件把编码后的 sRGB 字节直接乘 alpha，再当成线性预乘内容。滤波和 mip 也要遵守同样约定。浏览器默认 premultiply/colorSpaceConversion 不能替代一条经过验证的规范化链路。[R14]

预乘 source-over 的 RGB 为：

```
out_rgb = src_rgb_premul + dst_rgb_premul * (1 - src_alpha)
```

sRGB 的解码与最终编码各一次。wgpu 仅在相应 sRGB view 上执行对应转换；普通 Unorm 输出不能被当成自动获得正确 sRGB 呈现。[R1]

黄金样例至少包含半透明黑白边缘、渐变、线性转场中点、文字覆盖、遮罩、同色直出与离屏往返，以及两种后端对比。质量阈值区分预期的压缩误差和语义错误。

## 11. 文字与 UI 清晰度

cosmic-text/glyphon 是候选基础：前者处理文字塑形/布局，后者将字形缓存接入 wgpu；需要固定兼容版本并验证 WebGL2 路径、注音与目标字体，不把库名称当成完整日文排版保证。[R18][R19]

第一版正文优先测试覆盖率位图字形，不默认把所有中日文变成 MSDF。大标题/特殊缩放可单独评估其他方案。

三个缓存分开：

1. Layout：文本版本、语言、插值内容、字体集合、字号、布局宽度、排版规则、主题/视口代次。
2. Glyph bitmap：face/glyph、实际像素字号、子像素位置桶、渲染方式。
3. GPU atlas：位图身份、设备代次、图集位置与租约。

一句对白进入前完成整句排版和所需字形准备；打字机只更新揭示状态。不能每出现一个字就重新塑形整段。当前页面字形固定引用，回看仅布局可见窗口及小范围 overscan。

字体文件和字形缓存不是同一种分片。可以按语言/章节发布经过正确子集处理的字体，但必须保留塑形依赖、标点、动态文本与缺字补充方案。Rust 文字栈需要自己的可读字体字节；CSS 加载 WOFF2 不意味着 Rust 自动能读取它。基础路径可用经过子集化的 TTF/OTF；使用 WOFF2 时需要明确解压支持。[R20]

最终输出分辨率与舞台内部渲染分辨率独立。舞台/特效可以降低分辨率，UI/正文在最终输出尺度绘制，不与舞台一起先缩小再放大。输出本身仍受像素和内存上限限制，不承诺所有手机都以完整 DPR 渲染。

动态降档使用滞回和稳定边界。默认先减少非关键预测/装饰特效成本，再调舞台分辨率，最后才降低整体帧率；不能因一帧尖峰让画质反复跳动。

## 12. 帧调度、音频与恢复

### 12.1 按可见变化调度

| 状态 | 调度 |
| --- | --- |
| 入场、镜头、转场、UI 动效 | rAF 驱动可见帧 |
| 只有打字机 | 下一揭示点接近时请求帧，不必空转 |
| 完全静止对白 | 输入/刷新/尺寸变化才请求绘制 |
| 只有 BGM 在播放 | 音频宿主继续，不因此强制重绘 |
| 菜单/资源屏障 | Story 按契约暂停，UI 继续 |
| 后台页 | 显式暂停；回前台不补播墙上时间 |

浏览器后台会节流计时器并通常停止 rAF，需要可见性状态驱动，不能仅靠每帧 dt 猜测。[R21]

不假定 surface 的下一张图像保留上一帧。每次实际呈现应绘制完整结果，或使用自身持有的持久离屏缓存。停止请求新帧不同于下一帧只画变化矩形。

### 12.2 音频

短语音/音效采用受预算管理的 PCM 缓冲；长 BGM 根据循环精度选择流式媒体或有上限的缓冲/分块播放。流式媒体不自动保证无缝循环和逐采样恢复。[R8]

AudioBuffer 的 32-bit float PCM 意味着 48kHz、双声道、180 秒音频的基础样本约 65.9 MiB，不要用压缩文件大小做 PCM 预算。[R22]

语音准备与对白同一 Cue；自动阅读根据实际媒体契约等待。Story 与音频调度时钟保持映射；设备/页面恢复时重新锚定。普通 loading 不暗改 BGM 的时间语义。

### 12.3 错误恢复

Device lost：暂停 Story -> 增加 device epoch -> 作废 GPU 依赖 -> 重建当前必要资源 -> 重绘逻辑场景 -> 等待玩家继续。GPUDevice.lost 提供对应故障信号；旧设备的资源不能直接给新设备继续使用。[R23]

下载失败保留当前画面，按错误类型重试，允许返回菜单。准备组失败不能显现半套 Cue。内容 hash 不匹配不得进入有效资源缓存。读档、新章节与新设备的迟到回调不能修改当前执行，但无害且校验通过的内容字节可以进入内容缓存。

存档缩略图：从已完成画面派生小尺寸图，再异步回读；截图失败不应使逻辑存档失败。不要让 GPU readback 卡住阅读输入。

## 13. HTTP 缓存、可选离线与版本一致性

内容哈希资源使用长期 HTTP 缓存；入口/当前清单重新验证。运行会话锁定 manifest 版本，更新不混用新旧内容。HTTP 缓存与 CacheStorage 是两种机制。[R24][R25]

CacheStorage 可以由 Window/Worker 使用，不必须先引入 Service Worker；它也不会自动遵循 HTTP 缓存头或替你过期淘汰。[R25]

首版在线流程稳定后，再增加显式章节离线缓存。离线状态应验证完整 manifest 所需资源；缓存满时停止后台缓存或清理自身冷资源，不能让离线失败阻断在线播放。存档与可再下载的资源由不同应用清理规则管理，但浏览器仍可能按 origin 清理数据，所以保留备份入口。[R26]

多标签页和更新时不能直接删除仍在被其他会话使用的版本。首次只做保守版本保留策略即可，不需要复杂通用包管理系统。

## 14. 建议的模块接口

```
DemandPlanner       计算 Required/Near/Speculative 需求
PrepareCoordinator  DAG、联合预算、租约、完成与失败
AssetStore          内容身份与不同表示的缓存
DecodeService       浏览器图片与受控字节解码路径
UploadScheduler     GPU owner 的分帧创建与上传
TextPreparer        布局、字形和图集准备
PipelinePreparer    管线键、预热与错误
Renderer2D          场景增量、pass 计划、提交
FrameScheduler      rAF、定时唤醒、静态休眠
WebHost             输入/媒体/存储/页面生命周期
```

Host 输出的是准备结果和完成信号，不回调任意剧情代码。PrepareCoordinator 请求 Runtime 提交已准备 Cue，而不是自己替 VM 执行分支。

## 15. 观测与验证

所有测量关联 CueId/资源身份/session+device epoch，分开时间点：需求产生、请求排队、下载、解码、上传、管线准备、ReadyLease、Activate、提交、GPU 完成，以及可获得的呈现代理数据。

首屏指标不能混同；CPU submit 耗时不是 GPU 耗时。GPU timestamp 仅在支持时启用并异步读取，缺失时报告“未测量”。Long Animation Frames 是可选补充，不是全浏览器的唯一帧指标。[R27][R28]

建议起始验收门槛：固定网络/设备下首句可阅读 P95 <= 4s；资源已准备时输入到下一次相关渲染提交 P95 <= 50ms；60Hz 活跃演出超过 33.3ms 的帧低于 0.5%；静态阅读没有无意义的持续帧提交。它们不是本提案已经达到的数值。

至少比较：无预取 vs 压缩数据预取 vs 完整准备；普通图片 vs KTX2；主线程 vs Worker；合成面缓存开/关；单/双分辨率；WebGPU vs WebGL2。其他变量保持相同。

测试场景包括快速点击、选项未定、来回读档、长回看、首次字形、弱网、失败重试、窗口/DPR/字号变化、转场峰值、后台恢复、设备失效。长时运行观察内存与持续帧稳定性，不能用十秒空场景证明热机表现。

## 16. 实现顺序

A. 可观测保真基线：普通图片、文字、基础 sprite/转场、WebGPU 主路径和兼容能力烟测；每阶段均可计时。

B. 完整准备：资源 DAG、联合预算、分层缓存、ReadyLease、上传调度、文字/管线预热。先解决已有资源仍会卡顿的问题。

C. 预测与按需刷新：受预算 lookahead、选择分支淘汰、静态帧休眠、双分辨率和字形缓存。

D. 条件优化：真实数据支持时再加入 KTX2、Worker 全栈默认开启、分离发布产物、更复杂缓存和离线。Worker 宿主接口应在 A/B 就保留，不能后期依赖每帧全状态复制。

完成标准不是采用多少新名词，而是：关键 Cue 提交前已经准备完整、提交后不做昂贵首用工作、准备不挤占当前演出、资源故障可恢复。

## 参考资料

以下均为项目官方文档、规范或浏览器维护者文档；查询日期 2026-09-20。实现时固定依赖版本，不能将 latest 页面当成永久 API 合同。

[R1] wgpu crate：<https://docs.rs/wgpu/latest/wgpu/>
[R2] MDN WebGPU：<https://developer.mozilla.org/en-US/docs/Web/API/WebGPU_API>
[R3] wgpu Limits：<https://docs.rs/wgpu/latest/wgpu/struct.Limits.html>
[R4] MDN OffscreenCanvas：<https://developer.mozilla.org/en-US/docs/Web/API/OffscreenCanvas>
[R5] MDN SharedArrayBuffer：<https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/SharedArrayBuffer>
[R6] MDN Transferable objects：<https://developer.mozilla.org/en-US/docs/Web/API/Web_Workers_API/Transferable_objects>
[R7] MDN instantiateStreaming：<https://developer.mozilla.org/en-US/docs/WebAssembly/Reference/JavaScript_interface/instantiateStreaming_static>
[R8] MDN Web Audio best practices：<https://developer.mozilla.org/en-US/docs/Web/API/Web_Audio_API/Best_practices>
[R9] wgpu Queue：<https://docs.rs/wgpu/latest/wgpu/struct.Queue.html>
[R10] MDN onSubmittedWorkDone：<https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/onSubmittedWorkDone>
[R11] Khronos KTX：<https://www.khronos.org/ktx/>
[R12] Basis Universal 官方项目：<https://github.com/BinomialLLC/basis_universal>
[R13] MDN WebGL best practices：<https://developer.mozilla.org/en-US/docs/Web/API/WebGL_API/WebGL_best_practices>
[R14] MDN createImageBitmap：<https://developer.mozilla.org/en-US/docs/Web/API/Window/createImageBitmap>
[R15] MDN copyExternalImageToTexture：<https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/copyExternalImageToTexture>
[R16] MDN createRenderPipelineAsync：<https://developer.mozilla.org/en-US/docs/Web/API/GPUDevice/createRenderPipelineAsync>
[R17] wgpu PipelineCache：<https://docs.rs/wgpu/latest/wgpu/struct.PipelineCache.html>
[R18] cosmic-text 官方项目：<https://github.com/pop-os/cosmic-text>
[R19] glyphon 官方项目：<https://github.com/grovesNL/glyphon>
[R20] fontdb 官方文档：<https://docs.rs/fontdb/latest/fontdb/>
[R21] MDN Page Visibility：<https://developer.mozilla.org/en-US/docs/Web/API/Page_Visibility_API>
[R22] MDN AudioBuffer：<https://developer.mozilla.org/en-US/docs/Web/API/AudioBuffer>
[R23] MDN GPUDevice.lost：<https://developer.mozilla.org/en-US/docs/Web/API/GPUDevice/lost>
[R24] MDN HTTP caching：<https://developer.mozilla.org/en-US/docs/Web/HTTP/Guides/Caching>
[R25] MDN Cache：<https://developer.mozilla.org/en-US/docs/Web/API/Cache>
[R26] MDN Storage quotas：<https://developer.mozilla.org/en-US/docs/Web/API/Storage_API/Storage_quotas_and_eviction_criteria>
[R27] MDN GPUQuerySet：<https://developer.mozilla.org/en-US/docs/Web/API/GPUQuerySet>
[R28] MDN Long Animation Frames：<https://developer.mozilla.org/en-US/docs/Web/API/PerformanceLongAnimationFrameTiming>
