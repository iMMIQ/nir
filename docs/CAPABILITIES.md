# 首版能力表

此表说明当前实现，附件中其他条款不会被默认为支持。未知指令、未知核心字段、非 `web-v1` 配置及多模块入口均拒绝加载。

| 类别 | 当前支持 | 边界 |
|---|---|---|
| 逻辑 | Bool/I32/String、局部槽、纯表达式、checked 算术、函数/返回、Branch/Switch/Goto | 无浮点剧情变量、脚本扩展或任意 JS |
| 操作 | Assign、Random、DraftPatch、TaskControl、DialogueContinue、ProfileMerge | Profile 为布尔事实的单调集合 |
| 终结 | Call/Return、Activate/Await/Interact、End/Fault | 单剧情流、单模块 |
| 任务 | frame/session/scene/interaction scope、锁存 Started/Marker/Finished、失败优先于取消的 All | 无 Runtime Worker |
| 图像 | Group/Sprite、层次顺序、裁切、cut/dissolve、x/y/scale/opacity 动画 | PNG；无旋转、滤镜、视频和独立 Group 混合模式 |
| 正文 | 固定字体、样式强调、参数隔离、换行、字素簇揭示、span Marker/Gate | 无 Ruby、NVL、富网页标记；示例字体为子集 |
| 选项 | 稳定 OptionId、可见/可用表达式、默认超时、交互实例校验 | 旧实例和重复输入丢弃 |
| 界面 | 标题、对白、选项、菜单、设置、回看分页、存读档、自动、已读快进 | Fluent 界面内嵌在 SDK |
| 语言 | zh-Hans/en 契约覆盖，正文下一次实例化生效 | 单模块的两个正文包启动时均可用；不提供远程语言包流加载 |
| 音频 | PCM16 WAV、循环 BGM、短音效、合成测试语音、独立增益、手势解锁 | 无流式压缩音频、真人配音；可听性需要人工设备检查 |
| 存储 | 三槽 IndexedDB、事务确认、修订冲突、导入导出、独立偏好/Profile | 精确发行兼容；不迁移旧存档；无云同步 |
| 恢复 | 候选先验证/准备、暂停提交、检查点回退、设备重建 | 无安全热更新 |
| 发行 | 实际 SDK/CLI 身份锁、静态哈希对象、单会话固定发行、来源/体积报告 | 无 PWA、签名/CDN 调度或高级压缩优化 |
| 工具 | init/resolve/doctor/check/dev/build/test、Schema、架构检查 | dev 重建后手动刷新；CLI 本次产物为 Linux x86_64 |
| 平台 | 桌面 Chromium WebGPU、响应式、键盘/指针/触摸语义 | 无 WebGL2 回退；移动真机及其他浏览器未认证 |

资源账本、准备配方和缓存提供首版所需的分层准备与有界准入；没有实现附件中完整的通用 DAG 调度、跨模块加载、任意资源类型与高级缓存策略。v0.1.0 的实际测试列在 TEST-REPORT.md，后续有界事件队列、共享预算、独立暂停令牌、取消和分块上传的验证见 [引擎稳定性进展](ENGINE-STABILITY.md)；逐请求终态预留、迟到存读档回执及交错压力测试见 [请求生命周期进展](REQUEST-LIFECYCLE.md)。

结构化错误、作者来源定位、显式启用的脱敏阶段追踪与重复测量见 [诊断说明](DIAGNOSTICS.md)。部分旧工具错误仍没有精确来源；GPU 时间、物理内存和真实硬件性能尚未验收。
