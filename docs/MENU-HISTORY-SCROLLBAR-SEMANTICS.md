# 图片历史滚动条

`ui.menu-history-scrollbar.v1` 增加 `history_scrollbar` 菜单元素。它引用同页的一个 `history_flow`，依赖已有菜单元素、服务和连续历史能力。滚动位置由 Engine 的连续历史布局持有；控件不创建第二份历史，也不写剧情、Profile 或存档。

## 声明

```toml
[[image_menus.system.elements]]
id = "history-scroll"
rect = [700, 120, 32, 360]
content = { type = "history_scrollbar", window = "records", label = "History scroll", thumb_height = 24, arrow_height = 16, line_step = 45, track = { asset = "skin.track" }, thumb = { asset = "skin.thumb", hover_asset = "skin.thumb-hover", pressed_asset = "skin.thumb-pressed", disabled_asset = "skin.thumb-disabled" }, decrease = { asset = "skin.up" }, increase = { asset = "skin.down" } }
```

每页最多一个滚动条，且目标必须是同页的连续历史窗口。四个部件依次为轨道、滑块、向前箭头、向后箭头；每个部件必须提供普通图片，可分别提供悬停、按下、禁用图片。所有图片都进入页面准备、引用校验与资源闭包，必须为 Image。运行时不拆分图片状态条；来源状态图片的拆分属于转换器工作。

箭头位于两端，轨道排除箭头高度，固定高度的滑块在轨道中按 `offset/max` 定位。thumb_height 和 arrow_height 为有限值且至少 1，两倍箭头高度加滑块高度必须小于声明高度；line_step 为有限的 1–8192。部件随舞台和元素／祖先缩放；line_step 另随读者字号比例缩放。页面四个图片部件占四个元素绘制预算，整体仍受 256 上限约束。标签非空且最多 1024 字节。

父变换、透明度、裁切、显隐和启用条件沿用菜单元素契约。部件保持元素声明顺序，绘制与命中共用投影；后面的控件及禁用命中区可以覆盖滚动条。没有可滚动范围、目标隐藏／禁用、准备中、加载中或完全裁切时不接受滚动输入。

## 图片状态与输入

禁用图片优先，其次为按下、悬停、普通。缺少按下图片时使用悬停图片，再回退普通图片；缺少禁用图片时使用普通图片并把 RGB 乘以 0.35。父透明度和裁切始终保留。向前箭头在起点禁用，向后箭头在终点禁用。

左键按下箭头立即按 line_step 移动一次；按下滑块之外的轨道，按目标窗口的 page_step 向点击方向移动一次。当前没有按住箭头／轨道自动重复契约。按下滑块不跳动，持续拖动保留按下位置相对于滑块中心的偏移，按有限的连续比例定位并在范围两端钳制。拖出控件仍可继续拖动；取消不会撤回已经接受的移动。

滚动请求沿用 `menu_history_scroll`，另带可选 `control`：图片控件请求必须指定自己的控件 ID。line 只接受 -1/1 且必须来自有效滚动条；viewport 的 step 请求不带 control。page 和 position 保留既有校验。请求携带 instance、revision、window、layout，Engine 提交前重新投影并复查当前权限；每次接受移动刷新布局版本，旧请求无法再次使用。

拖动还冻结 session／interaction 和控件几何。自身接受的移动更新捕获版本；外部滚动、字号／宽度重排、几何／裁切变化、隐藏／禁用、关闭页面或换会话均取消旧捕获。宿主保留已捕获手势的消费状态直到松键，避免松键成为新页面的点击。后台、失焦、指针取消和设备恢复显式取消捕获。

轨道提供稳定语义 ID 的垂直 slider，箭头提供各自的按钮语义。轨道焦点下 Up／Left 向前、Down／Right 向后，Home／End 定位两端；PageUp／PageDown 继续使用窗口 page_step。辅助语义中的值、范围、方向及命中位置与画面共用当前投影，焦点保留不放宽提交版本检查。

## 平台与来源边界

Web 与原生宿主接入同一 Engine 手势接口。Web 使用指针捕获并在画布外继续拖动；原生离开窗口时取消。Linux 原生包可编译，Windows 真机仍待验收；软件 WebGL2 验证不代表硬件 WebGPU 或设备认证。

这项能力为自动迁移提供图片部件、几何和输入语义。LiveNovel 草稿已有检查后的状态条拆分、像素范围关系、原图边框及父页返回规则，见 [导入工具](IMPORT.md)。按住重复、scenario-page 间隔、完整格式器和原作播放对照仍待完成。不能仅凭图片尺寸猜测状态顺序，也不能把 NIR 记录间距等同于来源分页间距。
