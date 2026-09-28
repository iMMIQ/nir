# 场景方向擦除

StagePresent 保留唯一任务、资源准备及前后场景所有权，增加可选 transition。缺省与旧文件仍为 dissolve；非默认 wipe 要求 `stage.wipe.v1`，编译器按实际使用推导。纹理阈值遮罩使用下节的独立能力；消息根/UI 根与动态输入继续作为计划后续增量。

```json
{"type":"stage_present","scene":"next","duration_us":"600000",
 "transition":{"type":"wipe","direction":"left_to_right","softness":0.2}}
```

方向为 left_to_right、right_to_left、top_to_bottom、bottom_to_top，表示目标画面逐步出现的方向。softness 缺省 0，有限范围 0..1，表示归一化合成表面的软边宽度。表面为现有转场双输入渲染目标，覆盖完整视口，包括场景留白；不会把源引擎 vague/ramplen 不经换算地填入此字段。

进度 p 来自原任务 elapsed/duration；开始/结束精确输出原/目标画面。设沿方向的归一化坐标为 c，软边宽度 s：s=0 时 c<=p 显示目标；s>0 时阈值 t=p*(1+s)-s/2，目标覆盖率为 1-smoothstep(t-s/2,t+s/2,c)。两输入沿用现有线性、预乘 alpha 合成路径，只替换混合覆盖率，不叠加两份目标 opacity。GPU 实现与纯函数参考式共同保留。

输入场景冻结和 StagePresent 的互斥、Finish/Cancel、scope 清理政策沿用旧契约；零时长直接提交终点。玩家 reduced_motion 沿用旧的省略视觉转场路径，不重写任务控制流。暂停冻结任务进度；恢复使用同一任务中的方向、软边、前后场景与 elapsed，不重新从零开始。快照伪造与当前发行定义不一致的 transition 会被拒绝。

不增加媒体资源或第三张离屏纹理；联合资源预算沿用现有两张转场目标。此能力不升级源/运行时/快照主版本，新增字段缺省可读取旧文件；不支持把新声明交给不支持该能力的旧播放器。

WebGL2、硬件 WebGPU、Windows 实机分别记证据，共享 shader 不等于全平台已认证。当前转换器尚未推导来源擦除参数，具体来源映射需独立验证。

## Alpha 纹理阈值遮罩

`stage.mask.v1` 在相同双输入表面上使用图片的 Alpha 数据作为阈值，不增加第三张离屏合成目标。

```json
{"type":"stage_present","scene":"next","duration_us":"600000",
 "transition":{"type":"mask","asset":"mask.pattern","channel":"alpha",
               "invert":false,"softness":0.2}}
```

asset 必须是已声明的 Image；channel 当前只允许 alpha，其他值拒绝加载。遮罩按归一化双输入表面拉伸，最近邻采样、边缘 clamp，不循环、不做颜色空间转换。极性默认低 Alpha 值先出现目标，invert=true 则使用 1-alpha。softness 与方向擦除共用有限 0..1 软边和精确端点公式，以所取数据值替代方向坐标。

选择 Alpha 是数据约定：普通图片 RGB 上传会做颜色空间转换与预乘，不能被当作未修改的灰度数据。源工程的灰度遮罩须由转换器把标量写入 Alpha；当前没有自动来源映射，不把普通灰度 PNG 的不透明 Alpha 当成灰度值。测试图片的 RGB 故意与 Alpha 不一致，用来检查没有误读颜色通道。

遮罩加入 cue 媒体闭包、发行根索引、模块消费者、激活配方、运行中资产留存及读档目录/媒体准备。runtime 拒绝缺少遮罩的配方，按已有图片尺寸与解码/驻留成本记账；转场结束后释放活动留存，恢复运行中任务时重新准备。图片与前后场景联合准备成功后才提交，不在播放过程中偷偷请求遮罩。字段、方向/极性/软边都作为任务定义冻结并接受恢复一致性校验。

此项只支持冻结场景根。消息/UI 根、动态/live 输入、来源图像通道与 vague/ramplen 的精确换算仍待后续交付。硬件 WebGPU 和 Windows 实机仍需单独认证。
