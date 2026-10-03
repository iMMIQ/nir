# P0 验收样例：回想图集 · Replay Atlas

NIR-NEXT P0 验收样例之二（锁定回想菜单）。完整原创中性内容；覆盖：

- 标题图片菜单（自绘按钮/hover 图）与回想图集子菜单；
- 回想条目以 profile key 守卫：剧情授予 `atlas.north`，`atlas.south` 始终未授予（保持锁定态展示）；
- 回想入口函数以 `replay_completed` 结束并返回标题；锁定条目在播放器层拒绝执行（见仓库 Player 回归）；
- 菜单页效果：点击音、循环页面音乐、进入/关闭转场与进入边界逐元素动画。

```sh
novelc check --locked
novelc test
novelc build --locked
```

剧情路线 tour 对应 `tests/scenarios/`；回想入口由仓库集成测试直接驱动。素材与字体由 `scripts/make_p0_examples.py` 重建；验收映射见 `docs/NIR-NEXT-P0-BASELINE.md`。
