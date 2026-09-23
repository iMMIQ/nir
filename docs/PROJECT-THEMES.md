# 作品配置与主题组件契约

本阶段实现 NIR-0006 的作品默认设置、配置来源报告和受限主题替换，以及 NIR-0001 的语义槽投影。适用于本分支构建的 SDK；已发布的 v0.1.0 下载包不包含这些新增能力。

`game.toml` 登记引擎预设和作品输入：

```toml
[engine]
api = "nir-player/0.1"
capability_profile = "web-v1"
runtime_preset = "web-standard"

[inputs]
# 其余模块、资源目录等输入照常登记。
theme = "themes/rain/theme.toml"
player = "config/player.toml"
```

当前仅支持 `web-standard`。省略时使用这个默认值，其他值报 `E_RUNTIME_PRESET`。没有任意覆盖层或深合并；`game.local.toml` 不参与作品语义。`inputs.player` 可省略，旧的颜色 JSON 主题入口仍受支持。输入路径与资源路径使用相同的工程边界检查，包括符号链接目标。

## 作品默认设置

```toml
format = 1
[defaults]
font_scale = 1.0
bgm_volume = 0.3
voice_volume = 0.8
sfx_volume = 0.5
reduced_motion = false
auto_delay_us = "1200000"
prefetch_content = true
```

`font_scale` 范围为 0.8–1.5，音量为 0–1，`auto_delay_us` 使用十进制微秒字符串，范围 100000–30000000。省略字段使用 SDK 默认值。自动阅读在一行揭示完成后等待这个基础时长，加上每个 Unicode 字符 20000 微秒，并继续等待活动语音结束；暂停时不累计时间。逐字揭示速度仍由 Cue 的 `reveal_us` 控制，配置不能释放 Gate 或取消剧情任务。

`prefetch_content` 默认 `true`。剧情稳定等待时，播放器可沿正常成功路径预取下一个跨模块调用的静态包、代码及当前正文语言；遇到分支或选项停止预测。每次最多一个请求、2 MiB，只使用内容缓存空闲额度，不预取媒体或标题后的剧情。设为 `false` 可保持严格的执行到达后下载。该字段是作者配置，不属于用户偏好或存档；失败不打断剧情。具体预算和恢复行为见 [分块内容与驻留契约](CONTENT-RESIDENCY.md)。

首次访问使用作品默认偏好；浏览器语言只在作品支持时采用，系统“减少动态”会补充开启相应偏好。已保存的玩家偏好优先，包含玩家明确关闭“减少动态”的选择。自动等待基础时长属于作品规则，当前没有对应玩家设置控件。存档恢复或新游戏不会覆盖独立保存的玩家偏好。音频宿主使用播放器验证后的音量，不再自行填入固定默认值。

## 主题与组件

`themes/rain/theme.toml`：

```toml
format = 1
id = "theme.rain"
base = "builtin.reader"
tokens = "tokens.json"

[slots]
"dialogue.main" = "builtin.dialogue.top"
"choice.main" = "builtin.choice.compact"

[dialogue]
height = 240.0
padding = 24.0
font_size = 23.0

[choice]
width = 520.0
item_height = 58.0
```

| 槽 | SDK 内置组件 | 行为 |
|---|---|---|
| `dialogue.main` | `builtin.dialogue`（默认） | 底部对白框 |
| `dialogue.main` | `builtin.dialogue.top` | 顶部对白框，给系统工具栏留出空间 |
| `choice.main` | `builtin.choice`（默认） | 选项间隔 14 像素 |
| `choice.main` | `builtin.choice.compact` | 选项间隔 6 像素；不缩小触摸目标 |

尺寸使用视口 CSS 像素。对白 `height` 为 220–320，`padding` 为 12–32，基础 `font_size` 为 18–28。选项 `width` 为 360–680，`item_height` 为 48–72。窄屏会缩窄选项、调整对白尺寸；玩家字号倍率作用于对白与选项；`item_height` 是选项最小高度，实际文字更高时自动扩展。参数不能指定动作、写 VM 变量、禁用系统菜单或移除恢复出口。

颜色 JSON 只接受 `background/panel/accent/text/muted` 五组 RGBA。通道为有限的 0–1，alpha 至少 0.5；text/panel 和 accent/background 的基础 RGB 对比度至少 4.5:1。该检查只是基础颜色约束，不代表半透明叠加到任意场景后都满足可访问性要求，仍需检查真实画面。

组件共用引擎的字素簇揭示、Gate、OptionId、启用状态、命中区域和辅助语义。键盘焦点按原有动作保留。所有正常界面仍由 Rust/wgpu 绘制，DOM 只映射辅助访问。主题不能注入 Rust、JavaScript、HTML 或自定义动作。未知槽、错误组件类型与未知字段直接拒绝；越界参数报 `E_THEME_PROPS`，对比度不足报 `E_THEME_CONTRAST`。

当前主题只支持这两个固定语义槽与封闭的组件清单，不是通用 UI 语言。尚未实现任意每条 Cue 的 Props、主题图片/字体资源、页面组合、标题页替换、主题包继承或第三方组件。主题资源字段会作为未知字段拒绝；标题场景资源仍由既有运行依赖收集器处理。长对白与大量选项现有按实际字体尺寸的滚动阅读，见 [作者阅读与预览](AUTHOR-READING.md)。这仍不代表无限文本、任意主题或移动真机均已验收。

## 查看实际值与来源

```sh
./novelc -p my-story config
./novelc -p my-story check --locked
./novelc -p my-story build --locked
```

`config` 输出每个已解析字段的值和来源，如 `config/player.toml#/defaults/font_scale` 或 `builtin:web-standard`。构建报告 `reports/build.json` 也保留此表。运行 Program 只包含解析后的配置，不包含作者文件路径和来源表。作品相对路径可以出现在作者报告中，开发机绝对路径不会加入报告。

修改主题或默认设置后，可以继续使用同一个 `game.lock` 和配套 SDK 构建；新的运行配置会改变作品修订及发行身份。仍执行精确发行存档兼容检查。Schema 覆盖 `game`、`theme`、`theme-tokens`、`player` 和运行 `program`，数值边界、组件语义等还需运行 `check`，不能只依赖编辑器 Schema 校验。

验证记录见 [本阶段测试报告](validation/project-themes/README.md)。
