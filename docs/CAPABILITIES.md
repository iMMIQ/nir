# 首版能力表

此表说明当前实现，附件中其他条款不会被默认为支持。未知指令、未知核心字段及非 `web-v1` 配置均拒绝加载。多模块契约与验证范围见 [模块工作流](MODULE-WORKFLOW.md)。

| 类别 | 当前支持 | 边界 |
|---|---|---|
| 逻辑 | Bool/I32/String、局部槽、纯表达式、checked 算术、函数/返回、Branch/Switch/Goto | 无浮点剧情变量、脚本扩展或任意 JS |
| 操作 | Assign、Random、DraftPatch、TaskControl、DialogueContinue、ProfileMerge | Profile 为布尔事实的单调集合 |
| 终结 | Call/Return、Activate/Await/Interact、End/Fault | 单剧情流；跨模块通过具名导出调用 |
| 任务 | frame/session/scene/interaction scope、锁存 Started/Marker/Finished、失败优先于取消的 All | 无 Runtime Worker |
| 模块 | 多模块命名空间、导出链接、共享变量、函数体/正文哈希分包、执行与恢复前准备 | 静态目录与媒体按需准备、有界预取、租约保护及驱逐；无 Edition 或跨发行存档转换 |
| 图像 | Group/Sprite、层次顺序、裁切、cut/dissolve、x/y/scale/opacity 动画 | PNG；无旋转、滤镜、视频和独立 Group 混合模式 |
| 正文 | 注册字体、样式强调、参数隔离、换行、字素簇揭示、span Marker/Gate、已揭示长文翻阅 | 无 Ruby、NVL、富网页标记 |
| 翻译维护 | 源/契约/语义修订、契约摘要、已读语义身份、逐文本状态、显式复核、旧源迁移 | 简中/英文、逐模块修订；不迁移跨发行存档，详见 [文本修订](TEXT-REVISIONS.md) |
| 字体编译 | 静态 OTF/TTF/TTC face、subset/full、UI/正文逐语言有序 FontPlan、覆盖检查、共享字体字集去重、塑形闭包、内容缓存、许可打包 | 无可变/彩色字体、运行时补字；详见 [字体说明](AUTHOR-FONTS.md) 与 [语言/字体计划](LOCALE-FONTS.md) |
| 选项 | 稳定 OptionId、可见/可用表达式、默认超时、交互实例校验 | 按实际文字高度排版和裁切滚动；旧实例和重复输入丢弃 |
| 界面 | 标题、对白、选项、菜单、设置、回看分页与单条长记录翻阅、存读档、自动、已读快进 | Fluent 界面内嵌在 SDK |
| 作品配置/主题 | `web-standard`、player 默认设置、字段来源报告、dialogue.main/choice.main 内置组件替换 | 固定槽、受限 Props；无任意组件、主题资源或页面组合 |
| 语言 | zh-Hans/en 独立 UI/正文偏好与字体计划；候选准备后原子切换；正文下一实例生效，已存在的对白/选项/历史冻结身份 | 正文按模块/语言获取并校验；不提供繁简自动回退或多文字系统认证；详见 [语言/字体计划](LOCALE-FONTS.md) |
| 音频 | PCM16 WAV、循环 BGM、短音效、合成测试语音、独立增益、手势解锁 | 无流式压缩音频、真人配音；可听性需要人工设备检查 |
| 存储 | 按游戏/profile/发行隔离的三槽 IndexedDB、事务确认、修订冲突、导入导出、历史发行入口、独立偏好/Profile | 快照要求相同发行身份；无云同步 |
| 恢复 | 候选先验证/准备、暂停提交、检查点回退、设备重建 | 无安全热更新 |
| 发行 | 实际 SDK/CLI 身份锁、固定发行启动入口、stage/verify/promote/rollback、本地与 URL 校验、来源/体积报告 | 无 PWA、签名/CDN 调度或高级压缩优化 |
| 工具 | minimal/web-basic 模板、init/resolve/config/doctor/check/dev/build/test、text status/update/review/migrate/recover、Schema、架构检查 | dev 监听、候选构建与完整重载；CLI 本次产物为 Linux x86_64 |
| 平台 | WebGPU/WebGL2 自动选择、响应式、键盘/指针/触摸语义 | 桌面 Chromium 双后端、Firefox WebGL2 验收入口；移动端/Safari 实机验收后续安排 |

资源账本、准备配方和缓存提供首版所需的分层准备与有界准入；已加入函数体与正文的跨模块按需获取；没有实现附件中完整的通用 DAG 调度、任意资源类型与高级缓存策略。静态声明目录已分包按需加载，字体仍为逐语言计划。v0.1.0 的实际测试列在 TEST-REPORT.md，后续有界事件队列、共享预算、独立暂停令牌、取消和分块上传的验证见 [引擎稳定性进展](ENGINE-STABILITY.md)；逐请求终态预留、迟到存读档回执及交错压力测试见 [请求生命周期进展](REQUEST-LIFECYCLE.md)。

结构化错误、作者来源定位、显式启用的脱敏阶段追踪与重复测量见 [诊断说明](DIAGNOSTICS.md)。部分旧工具错误仍没有精确来源；GPU 时间、物理内存和真实硬件性能尚未验收。

作品默认设置和主题契约见 [作品配置说明](PROJECT-THEMES.md)。

长对白、大量选项和开发预览的操作与边界见 [作者阅读与预览](AUTHOR-READING.md)。

M4 的发行操作、存档隔离、后端选择和验收入口见 [发行与桌面渲染](M4-RELEASE.md)。
