# 编写与维护作品

先用 `novelc init my-story` 创建独立的最小双语作品；需要场景/声音/Gate 范例时用 `--template web-basic`。`game.toml` 只登记作品输入；本地输出放在 `.nir/`、`dist/`、`reports/`。程序拒绝未识别字段，扩展能力不能靠加一个任意 JSON 字段启用。

作品默认偏好与对白、选项主题在独立文件配置；UI 与正文语言、逐语言字体计划在 `config/locales.toml` 配置。用 `novelc config` 查看支持的最终配置值和逐字段来源，主题见[作品配置和主题契约](PROJECT-THEMES.md)，语言与字体见[独立语言上下文](LOCALE-FONTS.md)。

## 从一句对白开始

示例的 `intro` 文本由 `texts/contracts.json` 约定源/契约/语义修订、参数和 Gate，两个语言包分别提供同一文本 ID 的 `TextDoc`。结构包含 `text`、`break`、`param`、`gate` span；每个 span 都有稳定 ID，揭示完成时锁存同名 Marker。`gate` 会阻止跨越，需剧情中的 `DialogueContinue` 明确继续。

Cue 中的 `dialogue` effect 指向文本 ID，同时给出 `reveal_us`。块末尾 `activate` 准备并开启 Cue，下一个块的 `await` 等待 `finished` 或某个 Marker。示例 `letter → gate → bell → resume_letter` 展示了 Gate、音效等待与继续正文。

时间写字符串，例如 `"300000"` 表示 0.3 秒。JSON 类型采用显式 `type` 标签。变量仅支持 Bool、I32、String；表达式不会修改变量。I32 溢出会在原位置触发 Fault。

`scenes` 中无图像、尺寸为零的节点可作为 Group。位置与 clip 均使用节点局部坐标；`scale` 为统一非负缩放。子节点继承父级变换/透明度/裁切。`order` 对兄弟排序，整个子树保持连续。

## 阅读较长内容

正文可以包含长段落和显式换行；窗口高度不会限制整段的逻辑长度。播放器支持翻页浏览已揭示文字，选项按实际文字高度扩展，并在数量较多时滚动。编辑期间运行 `novelc dev`，保存文件会自动验证、构建和完整重载，编译失败则保留原预览。具体操作、进度保留边界和监听范围见 [长内容与开发预览](AUTHOR-READING.md)。

## 身份与翻译

- 函数、块、操作、Cue、场景、任务、选项、正文与 span 均使用可读稳定 ID。
- 翻译不可改变参数集合与类型、Gate 次序。修改源文后运行 `text update --id <id> --meaning preserve|bump`，逐语言检查后运行 `text review --id <id> --locale en`；`text status` 汇总缺失、过期和未复核项。
- 切换语言只改变未来实例；已经打开的对白、选项、历史不重新翻译。
- 改动剧情后重新构建会产生新的发行身份；本首版拒绝把旧发行存档带入新发行。
- 声明新图片时填写实际尺寸。素材路径相对 catalog；路径必须留在工程根内。
- 最小模板从母版自动裁剪新增字符；字体配置、覆盖、动态预留和许可见 [字体编译](AUTHOR-FONTS.md)。

修订、迁移、已读身份和中断恢复的完整说明见 [文本修订与翻译维护](TEXT-REVISIONS.md)。

`schemas/*.schema.json` 用于编辑器提示；`check` 还执行跨文件引用、类型、确定赋值、资源实际解码、字体覆盖与翻译契约检查，不能只用 JSON Schema 代替。

## 场景用例

用例位于 `tests/scenarios/*.toml`，登记在 `game.toml`。`await_choice` 驱动正式 Core 到指定选择，`choose` 使用 OptionId，结尾检查 outcome 和类型化的 `expect.variables`；旧 affection 断言兼容。示例有 `walk` 与 `stay` 两条路径。用例从合法新游戏入口运行，不任意拼造快照。

## 排查错误

| 错误族 | 处理方式 |
|---|---|
| E_SCHEMA / E_JSON / E_DUPLICATE | 检查未知字段、类型及重复身份 |
| E_PATH / E_PATH_ESCAPE | 改用工程内部的相对路径 |
| E_FONT_COVERAGE | 补充字体覆盖并保留字体许可 |
| E_LOCK_DRIFT / E_COMPILER_IDENTITY | 使用 SDK 配套 CLI；有意升级时显式 resolve |
| E_DIGEST / E_OBJECT_DIGEST | 重新部署完整对象；不要修改哈希 URL 下的文件 |
| E_SAVE_CONFLICT | 同槽被另一标签页更新，重新打开存档菜单再操作 |
| E_SNAPSHOT | 发行/版本不兼容或存档结构损坏，当前会话保留 |
| E_FUEL | 存在无限非悬挂循环，加入真实等待或结束路径 |
| E_WEBGPU | 使用支持 WebGPU 的桌面 Chromium 和安全来源 |
| E_BUDGET | 减小资源/舞台或拆减同时活动的视觉与声音资源 |

发布前运行 `check --locked`、`test`、`build --locked` 和发行校验脚本，再用实际发布目录跑浏览器测试。不要将开发服务器 HTML 回退响应当作丢失资源，也不要只测试源码预览而忽略最终静态目录。

## 静态发行的压缩传输

构建会为能够缩小的 WASM、JavaScript 和 JSON 不可变对象生成同目录 `.gz` 旁文件，使用固定时间戳的 gzip level 6。部署时一并上传。对象路径、发行清单、摘要和大小仍描述原始对象；旁文件不建立新的剧情或发行身份。

`novelc serve` 和作者预览按 `Accept-Encoding` 返回压缩表示，设置 `Content-Encoding: gzip`、`Vary: Accept-Encoding` 及压缩后的 `Content-Length`，保留原对象 MIME 和不可变缓存头。浏览器解压后，启动器与运行时继续校验原始长度和 SHA-256。无 gzip 支持或请求明确禁用 gzip 时返回原文件。

生产静态托管应开启对应的预压缩文件协商（例如 Nginx 的 `gzip_static on` 配合 `gzip_vary on`，需要服务器包含相应模块），或由托管服务动态压缩相同 MIME。需要保留原 MIME、正确的 `Content-Encoding` 和 `Vary: Accept-Encoding`。原始 URL 必须保持不变，不能将请求重定向到 `.gz` URL，也不能让 SPA 回退页面覆盖对象缺失错误。只上传旁文件而没有协商规则不会产生优化效果；托管不支持压缩时原文件仍可正常使用。

发布前运行 `python3 scripts/verify_release.py <发行目录>`；校验器会额外检查清单对象已有旁文件的解压结果。预览更新同样验证并同步旁文件后才切换 channel。
