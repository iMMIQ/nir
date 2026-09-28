# 音频实例、增益与停止

当前新增能力为 `audio.gain.v1` 和 `audio.stop.v1`。仍使用既有 Effect/Task、Activate/Await、scope 与准备提交，不增加另一条剧情执行流。

## 播放与音量

`Audio` 的 `gain` 为有限的 0–4，缺省 1。音频素材保留单位增益 PCM，实际输出为事件 gain × 玩家总线音量 × 实例包络。设置界面改变总线音量不能覆盖正在进行的包络，也不能释放菜单/后台的暂停所有权。超过满刻度的混音仍可能在设备输出端削波，事件增益并非自动限幅器。

## 淡出停止

```json
{
  "id": "music_stop",
  "scope": "session",
  "effect": {
    "type": "audio_stop",
    "target": "music",
    "duration_us": "500000"
  }
}
```

这是 Cue 中的一项 EffectDef。target 引用已建立的 Audio 句柄，在激活提交时绑定具体任务实例；随后同名句柄指向新声音也不改变此绑定。duration 为十进制微秒字符串，范围 0–60 秒。target 必须为同模块的音频任务，不能引用停止任务自身。

停止任务从目标当前包络线性淡至 0，期间目标 Audio 保持 Running 并保留资源；到期停止声音。等待 `music_stop.Finished` 表示停止操作完成；原 Audio 为 Cancelled，不能据此声称自然播完。

| 事件 | 结果 |
| --- | --- |
| duration=0 | 同次激活提交完成停止 |
| 目标提前自然结束 | Audio 保持 NaturalEnd，停止任务立即 Finished |
| 对已终止实例停止 | 停止任务立即 Finished，不复活声音 |
| 另一个停止任务仍拥有同一实例 | 拒绝为所有权冲突；候选提交回滚 |
| Cancel 停止任务 | 提交当前包络值，原声音继续，不恢复为 1 |
| Finish 停止任务 | 立即执行终点，停止原声音 |
| 直接 Cancel/Finish 原 Audio | 原声音立即结束，所属停止任务同步收尾，原因不冒充自然结束 |
| scope 退出或新会话 | 沿原 scope/会话清理硬停止，不等淡出阻塞清理 |

停止任务不能引用尚未建立的音频；同 Cue 内顺序建立的前一项 Audio 可以被随后 Stop 引用。未知目标类型、跨模块目标、非法时长和缺少能力声明在验证时拒绝；活跃实例和冲突在原子提交时再次检查。

## 恢复和后端

快照 v2 保存终态原因、停止目标实例、捕获值、包络基值、任务 elapsed、可选的设备播放位置 audio_position_us 与停止任务的 audio_device_elapsed_us。恢复验证引用、任务类型、单实例所有权及进度，并从剩余包络继续；不重新从单位增益开始。重复/迟到的音频结束回调不能改写已停止任务或新声音。

设备位置是观测元数据，不是第二份剧情时钟。宿主在动作提交和 Tick 前采样活动声音；Player 按时间域/会话过滤，Core 只接收运行中的 Audio 实例，拒绝重复任务和超限批次。观测不执行指令、不推进 elapsed 或里程碑。保存/恢复优先使用最近的设备位置；旧快照缺字段时沿用 elapsed。循环音频保存累计位置，起播时按素材长度取模。后台暂停和菜单暂停仍由原暂停所有权决定。此字段仍属于尚未发行的快照 v2，不要求作者声明新能力；新宿主和 WASM 必须来自配套 SDK。

这解决主线程停顿后立即进入菜单时，设备继续播放而剧情按输入优先暂停造成的 seek 偏差。停止任务另有可选的设备包络进度检查点：AudioEnvelope 命令携带具体停止任务 owner 和累计起点 elapsed_us；Web 按暂停感知的 AudioContext.currentTime 采样，原生按已产生采样帧采样。AudioPosition.envelope 回传 owner/elapsed_us，Core 仅接受仍拥有该 Audio 实例的运行中停止任务，进度必须不超过声明时长，不能倒退。先验证整批再修改，旧 owner 或会话记录不影响新声音。

读档与 Cancel 停止任务优先使用设备检查点的当前值，恢复剩余线性段从累计起点继续，避免听到的音量跳回剧情时钟推算值。常值替换清除设备 owner，迟到的旧 ramp 观测不能覆盖已提交值。旧 v2 快照缺检查点时仍使用原逻辑 elapsed。检查点仅接受 AudioStop，恢复验证其类型和进度；不新增作者能力或快照版本号，宿主与 WASM 必须配套。

这份观测不会推进 Story tick、任务逻辑 elapsed、里程碑或执行指令；AudioStop 的 Finished/Cancel、Await 和作用域规则不变。设备先淡至零而逻辑任务尚未结束时，保持静音直到原任务收尾，不把观测伪装成剧情时间推进。这不是音频设备与剧情时钟的全局同步承诺。

Web Audio 使用独立包络 GainNode；玩家音量与事件 gain 使用另一 GainNode。原生音频使用按采样帧推进的包络，两声道同帧使用相同值；暂停 sink 不消耗包络样本。设备输出延迟与 Story 时钟误差需按实际平台单独测量，不宣称样本级跨设备同步。

## 导入与验证入口

LiveNovel 适配将 BGM `STOPSND ... PASS` 转为不阻塞主流的停止任务；非循环语音在源页完成处以 50 ms 淡出停止。循环语音不执行该翻页清理。导入器复用每通道的停止句柄，避免长篇阅读积累无限任务名。

原生语义回归：`cargo test -p nir-core --test audio_contract`。采样包络及存储回归：`cargo test -p player-windows --lib`；Linux 执行该命令不代表 Windows 音频设备实测。

构建配套 SDK 后，`npx playwright test --config playwright.nir-next.config.js` 会生成中性工程并检查 WebGL2 的事件增益、连续淡出、暂停、存读档剩余包络及会话清理。该入口不使用私有游戏或外部媒体工具。Windows 真机和硬件 WebGPU 仍需独立验收。
