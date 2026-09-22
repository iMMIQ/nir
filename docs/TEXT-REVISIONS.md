# 文本修订与翻译维护

对应 NIR-0006 §8、NIR-0003 §3。本阶段实现作者文本修订、翻译状态和复核流程，不包含独立 UI/正文 LocaleContext、配音目录或跨发行存档迁移。需要从当前源码构建配套 SDK；旧版下载包不会自动升级。

## 日常编辑

`novelc init my-story` 创建的两个模板均已使用新格式。先修改源语言正文，例如 `content/main/texts/zh-Hans.json` 的 `intro`，再运行：

```sh
novelc -p my-story text status
novelc -p my-story text update --id intro --meaning preserve
# 阅读源文改动，修改或确认英文翻译后：
novelc -p my-story text review --id intro --locale en
novelc -p my-story check --locked
novelc -p my-story build --locked
```

`--meaning` 必须显式填写：`preserve` 表示作者判断这是不改变剧情含义的修订；`bump` 表示含义发生变化。例如拼写修正可以选择 preserve；选项含义或剧情意图改变应选择 bump。工具不会替作者判断语义。即使文字字节未变，也可用 bump 记录因剧情上下文变化产生的新含义。

`update` 更新源文修订记录，并让该 TextId 的所有翻译待复核；它不替换翻译内容，也不自动确认翻译。`review` 是作者的显式确认：翻译可以经过改写，也可以确认现有译文仍然合适。通过检查后更新翻译针对的源/契约版本和摘要。单独修改译文也会变成待复核，运行 review 即可，不必提升源文或语义版本。

不要手工抄摘要或修改已登记的版本号；由 CLI 完成。手动将译文版本号改成新值仍不能绕过复核。源文未登记、译文未复核、缺失文本、Gate/参数不匹配时，`check`、`build`、`test`、`resolve` 都拒绝继续。dev 同样拒绝新候选，保留当前有效发行和会话；修复后按原有行为完整重载，从标题开始。

## 三种修订与身份

作者契约 `texts/contracts.json`：

```json
{
  "intro": {
    "source_revision": 1,
    "contract_revision": 1,
    "meaning_revision": 1,
    "gates": [],
    "params": {}
  }
}
```

各语言的 TextDoc：

```json
{
  "intro": {
    "source_revision": 1,
    "contract_revision": 1,
    "spans": [{"type": "text", "id": "body", "text": "新的故事。"}]
  }
}
```

| 身份 | 意义与更新规则 |
|---|---|
| TextId | 同一个逻辑文本的稳定 ID；正文、角色名和选项标签均使用此模型 |
| source_revision | 当前源文版本；记录源文变化或显式含义变更时增加 |
| contract_revision | Gate、参数或语义契约版本；结构变化或 meaning bump 时增加 |
| meaning_revision | 已读语义身份；作者选择 bump 时增加，结构性契约修改必须 bump |
| contract_digest | 编译器对版本化契约编码、契约/语义修订、参数类型与有序 Gate 计算 SHA-256；仅生成到运行数据 |

普通源文改字并 preserve 时，只提升 source_revision，contract_digest 和已读键保持不变。Gate 增删/改序、参数增删/改类型必须选择 bump，这时三种修订都提升。正文 span 内容、ID、样式、换行变化也会被源摘要检测，不是只有纯字符串改动才算变更。JSON 缩进、对象键顺序和等价默认字段不构成正文变化。

翻译必须完整覆盖所有参数；参数允许按译文语序排列，同一参数可多次出现。Gate 必须保持声明的身份、数量和顺序。翻译新增未知参数、漏掉参数、重排 Gate 或重复 span ID 均不能被 review 放行。参数的 VM 类型仍由正式编译检查；review 不代表整个作品已通过所有验证。

## 修订记录与状态报告

模块声明新增：

```toml
text_contracts = "texts/contracts.json"
text_revisions = "texts/revisions.json"
```

`revisions.json` 是作者工程的必要输入，**需要提交到 Git**。它保存最近一次登记的源文摘要、修订、契约摘要及各语言确认的文档摘要。即使忘记修改整数版本号，源文改动仍会被发现。这是协作记录，不是签名或防篡改机制；不应手动编辑。合并冲突应保留正确的源/契约历史，再逐条复核，不能删除文件让工具自动接受全部翻译。

```sh
novelc -p my-story text status --json
```

报告包括 ready、文本/语言数量，以及每项问题的错误码、TextId、locale、项目内文件路径、JSON Pointer 和原因。缺失、过期、未复核、契约不符分别报告，顺序确定。status 是检查报告命令，成功生成报告返回 0，即使 ready=false；CI 应读取 ready 或执行 `check --locked`。正式检查会给首项错误的源码行列和修复提示。JSON 语法错误或整个输入文件缺失会直接失败，不能生成完整逐文本报告。

记录不进入运行发行；Program/Executable 中只包含验证后的契约、文本和生成的契约摘要。SDK/CLI 身份继续由 `game.lock` 锁定。修订和复核不改变 SDK 锁。

新增 TextId 时，在契约与源文写入初始版本 1，并运行 update 登记；补齐其他语言后逐个 review。删除文本时同步移除契约、全部语言和所有剧情引用；记录中旧 ID 保留为历史，不建议将它重新用于不同文本。单纯存在旧记录不会把已删除正文带入运行包。

## 旧工程迁移

```sh
novelc -p old-story text migrate --out new-story
novelc -p new-story resolve
novelc -p new-story check --locked
novelc -p new-story test
```

输出目录必须不存在，并且位于旧工程之外。迁移先验证旧文本 revision、参数和 Gate，再在临时目录生成候选，最后提交到新目录；原工程保持不变。不会复制 `.git`、`.nir`、构建输出或旧 game.lock，并生成当前 Schema。

旧 revision 映射为三种初始修订，TextId、正文、GameId 保留。现有合法翻译作为**迁移时的初始基线**导入，工具无法判断它们在旧流程中是否真的接受过人工复核；作者应在迁移前确认这一点。已使用新记录的工程不允许再次 migrate 来清除待复核状态。迁移只处理文本源格式，不自动修复其他无效作品配置。

研发阶段在 v1 上直接进行破坏性迭代：Program、Executable 和逻辑 Snapshot 的版本号保持 1；新运行数据声明 `text.revisions.v1`。旧结构的运行数据和快照因字段不兼容被拒绝；相同的 v1 数字不代表与早期开发构建兼容。项目、模块和发行清单外壳也仍保持原版本。仍只支持精确发行兼容，**不会把旧存档迁移到新发行**。

## 已读、历史与恢复

已读键现在使用 `read:<TextId>:<meaning_revision>`。修正错字且 preserve 不会仅因文本修订而丢失已读资格；含义或契约变化并 bump 会产生新的已读键。是否继续快进仍取决于既有 Profile 与播放器规则。

Dialogue 和 History 冻结 meaning_revision、source_revision、contract_digest、实际语言和实际内容。准备中的对白在语言偏好变化后也不重新生成。恢复检查这些身份，并保持当前对白/历史的原实例；源文修订机制不放宽发行匹配。

## 多文件写入与恢复

update/review 在写入前保留原文件字节、检测并发修改，使用完整写入后原子登记的 `.nir/text-transaction.json` 日志，再逐文件原子替换。成功后删除日志。若进程中断，构建拒绝继续：

```sh
novelc -p my-story text recover
```

recover 将该次工具写入整体回滚到命令开始前，保留作者在运行命令前已写下的文本。如果文件在中断后又被修改，恢复拒绝覆盖这些新编辑，需先保留它们并按日志处理。不要在另一条文本命令仍运行时执行 recover。单次日志上限 64 MiB；超限在写入前报错。没有提供多人协作服务、分布式锁或断电持久性认证。

当前语言范围仍为简中/英文、单模块。独立 UI/正文语言、VoiceKey/SpeakerId、多模块和译文导入导出平台留待后续阶段。验证见 [本阶段报告](validation/text-revisions/README.md)。
