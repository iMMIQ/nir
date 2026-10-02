# P0 验收样例：夜航日志 · Voyage Log

NIR-NEXT P0 验收样例之三（页签设置/存档/历史页面）。完整原创中性内容；覆盖：

- 单页三页签（设置/存档/历史）的局部状态页：enum/int/bool 局部值与 `set_local` 切换，
  `visible_when` 条件页，`text_local`/`text_preference`/`text_slot` 三种动态文字；
- 值控件：偏好滑条（字速/音乐音量）、`reduced_motion` 开关、局部 bool 开关与局部有界
  int 滑条（面板亮度），内置绘制，无图片依赖；
- 设置行以 `stack` 容器紧凑纵向排列（`ui.menu-stack.v1`）；
- 存档槽：局部槽位选择 + `save_slot`/`load_slot`（局部槽位解析，3 槽上限）；
- 历史页：`history_window` 分页窗口 + `history_page` 更早/更近；
- 标题页 `push_menu` 进入面板（`ui.menu-navigation.v1`），`menu_overlay` 使剧情内
  菜单键直达同一面板（`ui.menu-services.v1`）。

```sh
novelc check --locked
novelc test
novelc build --locked
```

剧情路线 tour 对应 `tests/scenarios/`；player 级驱动（页签、值提交、历史分页）见仓库
集成测试。素材与字体由 `scripts/make_p0_examples.py` 重建；验收映射见 `docs/NIR-NEXT-P0-BASELINE.md`。
