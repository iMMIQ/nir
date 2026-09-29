# Android 原生发行（实验性）

Android 版使用 Rust、winit（android-activity / NativeActivity，无 Java 代码）、wgpu Vulkan 渲染和 AAudio 音频（rodio/cpal）。它与 Web 版共用 `nir-engine`、Player、Core、文字排版和场景合成，不使用浏览器、JavaScript、WASM 或网络连接。

当前状态为**实验性**：交叉编译链路、APK 打包与签名已由 CI（含 Android 官方 `aapt2`/`apksigner` 工具）结构性验证，但尚未完成真机安装、运行、GPU 与音频的实测验收。真机证据补齐前，不声明正式的 Android 平台支持。

## 构建

SDK 侧（构建分发工具链的人，一次性）：任意主机安装 Android NDK（在 r27c 上测试），设置 `ANDROID_NDK_HOME`（或 `ANDROID_NDK_ROOT`），并安装 Rust 目标 `rustup target add aarch64-linux-android`。`cargo xtask sdk` 随后会把 `sdk/android/lib/arm64-v8a/libplayer.so` 一并产出；未发现 NDK 时跳过该部件（与 Windows 主机跳过 Linux 播放器相同）。播放器真实字节纳入 `game.lock`；缺少该文件的 SDK 明确拒绝 Android 构建（`E_SDK_ANDROID`）。

作者侧（作品目录内）：

```bash
novelc -p my-story resolve
novelc -p my-story build --target android --locked
```

作者机器不需要 Rust、Java、Gradle 或 Android SDK：`novelc` 内置二进制清单编码、zip 打包与 APK Signature Scheme v2 签名（`nir-apk`，ECDSA P-256）。v2-only 签名要求 minSdk ≥ 24，本播放器为 26（Android 8.0+，AAudio 所需）。

输出 `dist/full/android/` 包含 `Game.apk`、`NOTICE.txt` 和 `README.txt`。只分发 `Game.apk` 一个文件即可。

签名密钥首次构建时自动生成在作品 `config/android-signing.pem`（权限 0600）。包名 `one.nir.g<15 位十六进制>` 由 game_id 派生：重建同一作品是同一应用的更新安装，不同作品互不冲突。`versionCode` 固定为 1，`versionName` 为作品版本。**务必保留该密钥**：Android 拒绝以不同密钥签名的更新，丢失后只能卸载重装（存档随之删除）。密钥不会进入模板、发行物或 `copy_tree` 复制。

## 使用与存档

安装：`adb install -r Game.apk`，或在设备上打开 APK 文件并允许来自该来源的安装。要求 Android 8.0+（API 26）、arm64-v8a、Vulkan 驱动；没有 GLES 回退后端。触屏为单指针语义（按下/拖动/抬起/取消），返回手势或返回按钮等价于 Esc（打开/关闭菜单）。无系统文件对话框：导出的存档写入 `Android/data/<package>/files/exports/`（USB 可见），导入在此平台不可用（`E_DIALOG`）。

首次启动把 APK 内 `assets/data/**` 提取到应用私有存储（同名同大小的文件跳过，应用更新只重拷变更部分），此后走桌面版完整的内容摘要校验链。启动时还经 JNI 读取 `nativeLibraryDir`，对已安装的 `libplayer.so` 做整文件摘要校验，与桌面 EXE 校验同构。

存档位于应用外部文件目录 `NIR/games/<game-id-hash>/<profile>/releases/<release>/`，与 Web/桌面存档不共享，按发行摘要隔离。`adb install -r` 覆盖安装保留数据；卸载即删除。存档写入沿用原子替换与槽位修订检查。

暂停/恢复：系统挂起时先给存储写入一个有界窗口（至多 5 秒），随后在窗口销毁前释放呈现表面；恢复时在同一 GPU 设备上重建表面并继续，不重放资产、不重置剧情。进程在挂起后可能随时被系统杀死，存档一致性依赖上述每笔写入的原子性。界面缩放沿用全局 1–2 限制。

## 验证

```bash
cargo clippy --locked -p player-desktop -p player-android --target aarch64-linux-android --all-targets -- -D warnings
python3 scripts/verify_android.py my-story/dist/full/android
```

`verify_android.py` 做无设备结构验收：APK 为合法 zip、`lib/arm64-v8a/libplayer.so` 以 STORED 存放且摘要与原生发行清单一致、内容对象图完整、二进制清单字符串池含 NativeActivity 标识、v2 签名块存在；检测到 Android build-tools 时额外执行 `apksigner verify` 与 `aapt2 dump badging` 黄金标准校验。结果写入 `reports/android/`。

CI（`.github/workflows/android.yml`）在每次推送与 PR 上运行完整链路：NDK 交叉编译 → SDK 打包 → `novelc build --target android` → 结构与签名验收；证据与样例 APK 作为 artifact 保留，发行版附带示例 APK。示例 APK 使用 `NIR_ANDROID_SIGNING_SEED` 固定种子派生的公开示例密钥签名（仅用于示例产物），因此跨发行版字节可复现且可覆盖安装；作者自己的作品永远走 `config/android-signing.pem`，不受该种子影响。尚待补齐：真机安装与长时间运行实测、扬声器/蓝牙音频实测、Play Store 发布通道、多 ABI（armeabi-v7a/x86_64）、GLES 回退与 `versionCode` 自动递增。
