# P0 验收样例：夜灯书页 · Reading Lamp

NIR-NEXT P0 验收样例之一（对白/语音/转场 + Gate）。完整原创中性内容；覆盖：

- 页内 Gate（风声 chime、灯语 rest 两个 marker），Gate 后继续同一对白；
- 显式语音绑定 `dialogue_voice`（`sampled_remaining` 等待策略）与页尾 50 ms 语音淡出停止；
- 消息窗口样式化显隐（`dialogue_visibility`：wipe 揭示 / dissolve 隐藏）与舞台场景转场；
- 非单位事件增益（BGM 0.7）与固定 Auto 等待政策；
- 类型化选择结果：选项把声明的值写入 `kept` 变量后再分支。

```sh
novelc check --locked
novelc test
novelc build --locked
```

两条剧情路线（sunrise / rest）对应 `tests/scenarios/`。素材与字体由 `scripts/make_p0_examples.py` 重建；验收映射见 `docs/NIR-NEXT-P0-BASELINE.md`。
