# ZViewer

基于 Tauri 2.x 的精美快速看图软件。深色毛玻璃 UI，核心特色是**跨同级文件夹无缝浏览**与**RAW/动画格式支持**。

> 文档：`docs/需求报告与技术方案.md`（需求与技术方案）、`docs/UI设计草图.md`（界面唯一依据）、`docs/开发阶段规划.md`（项目状态与功能规划）、`docs/soft-candy-integration-guide.md`（专业版激活/埋点/更新接入契约）

## 技术栈

- **应用框架**：Tauri 2.x（Rust 后端 + WebView2 前端）
- **前端**：Vanilla TypeScript + Vite（无框架运行时开销）
- **包管理**：pnpm
- **图片解码**：rawler（RAW：CR2/CR3/NEF/ARW/DNG 等）+ image crate（GIF/APNG/WebP 动画拆帧）+ jxl-oxide（JPEG XL）

## 功能

- 跨同级文件夹无缝浏览（natord 自然排序，与资源管理器一致）
- 常见格式：JPG / PNG / GIF / WebP / BMP / ICO / SVG / AVIF / TIFF / JXL
- 看图观感：小图适应窗口不放大、高质量平滑缩放渲染、信息条实时缩放百分比、
  切图瞬切、RAW 解码中指示
- 相机 RAW：CR2 / CR3 / NEF / ARW / DNG 等（内嵌预览快显 → rawler demosaic 全量替换，首帧毫秒级）
- 动画格式：GIF / APNG / 动态 WebP 默认浏览器原生播放（首帧即时、零内存），
  帧控制（播放/暂停/上一帧/下一帧）按需拆帧
- 沉浸模式（F）、窗口置顶（T）、缩放/旋转/翻转、1:1/适应窗口
- 幻灯片播放（空格/▶，间隔 2s/5s/10s，自动跨文件夹，播放完自动停止）
- 预加载缓存：相邻图片后台预取，RAW 解码 LRU 缓存（切换 ≤50ms）— **专业版**
- 异步浏览：打开图片立即显示（只枚举当前文件夹），同级文件夹后台枚举，
  完成后自动刷新位置计数（加载中显示 "3/…"）

> **专业版**（预取缓存）通过 soft-candy 在线激活码解锁。授权协议见
> soft-candy `docs/api.md` §4.2/4.3。

## 专业版解锁

应用内点「缓存」按钮 → 解锁对话框 → 输入激活码。

- 激活码由 soft-candy 服务端绑定设备，签发 EdDSA JWT
- JWT claims：`iss/aud/sub/code/app/level/levelLabel/deviceId/features/iat/exp`
- 客户端内置当前产品对应等级的公钥，离线验签可用；启动时联网续验换新 JWT
- 激活/续验地址、等级公钥在 `src-tauri/tauri.conf.json` → `plugins.store` 配置
- 等级公钥支持 raw 32 字节 base64、完整 SPKI DER base64 或 PEM，客户端会自动解包

## 开发

```bash
pnpm install          # 安装前端依赖
python scripts/gen-test-images.py  # （可选）生成 test-images/ 测试图库（需 Python）
pnpm tauri dev        # 开发模式（前端热更新 + Rust 自动重编译）
```

**打开图片**：

- 命令行传参：`pnpm tauri dev -- test-images/A/10.png`
- 拖拽图片 / 文件夹到窗口任意位置
- 正式安装后支持右键「用 ZViewer 打开」，也可在 Windows 设置 > 默认应用中将 ZViewer 设为默认看图后双击打开（非抢占式关联，不劫持系统默认）

## 快捷键

| 按键 | 功能 |
|------|------|
| ← / → | 上一张 / 下一张（跨文件夹无缝衔接） |
| PgUp / PgDn | 上一个 / 下一个同级文件夹 |
| ↑ / ↓ | 放大 / 缩小（以图片中心为锚点） |
| 滚轮 | 缩放（以鼠标指针为锚点） |
| 双击 | 实际大小 ↔ 适应窗口 |
| R / Shift+R | 右旋 / 左旋 90° |
| H / V | 水平 / 垂直翻转 |
| 1 / 0 | 实际大小 / 适应窗口 |
| F / F11 | 沉浸模式切换（全屏纯黑，Esc/F 退出） |
| T | 窗口置顶切换 |
| 空格 | 幻灯片播放 / 暂停 |
| Esc | 退出沉浸模式 |
| 鼠标左键拖拽 | 放大后平移图片 |
| 鼠标侧键（后退/前进） | 上一张 / 下一张 |

## 构建

```bash
pnpm tauri build -- --no-bundle   # 仅产出可执行文件（快速验证）
pnpm tauri build                  # 完整打包（Windows NSIS 安装包）
pnpm build:linux                  # Linux 打包（deb + AppImage）
```

**Linux 构建前提**（Ubuntu/Debian）：

```bash
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file patchelf libxdo-dev libayatana-appindicator3-dev librsvg2-dev
```

**JXL 解码引擎（libjxl vendored 构建）的前提**：构建机需 `cmake` + C++ 工具链。

- Windows：优先用 PATH 上的 `clang-cl`（build.rs 自动比选最新版本；VS 18 的 STL 要求 clang ≥ 19），配合 Ninja。找不到 clang-cl 时回退 MSVC + 全局 `/arch:AVX2`（放弃 Haswell 之前的 CPU）。MSVC cl 直接构建会让 libjxl 的 SIMD 派发失效、解码慢 1-2 个数量级，不可用。
- Linux：`build-essential`（gcc/g++）即可。
- 构建机受限时可关闭特性仅用 jxl-oxide：`cargo build --no-default-features`（解码慢约 3 倍）。

- 产物位于 `src-tauri/target/release/bundle/`（`deb/` 与 `appimage/` 子目录）
- 自动更新（latest.json）在 Linux 上仅对 AppImage 生效，deb 需手动升级
- 发版：打 `v*` tag 后 CI 自动构建 Windows/Linux 双平台安装包并发布（矩阵并行构建，统一生成双平台 latest.json，见 `.github/workflows/release-tauri.yml`）

## 目录结构

```
src/                  # 前端代码
  main.ts             # 入口、缓存开关、幻灯片播放条接线、事件装配
  viewer.ts           # 渲染与变换（缩放/旋转/翻转，rAF 合并 60fps）
  presenter.ts        # 图片呈现：代次守卫、通道分发、预取池、帧条策略
  presence.ts         # 可见性 + 意图 + 节奏：浮层分组、键盘映射、幻灯片节拍器
  ui.ts               # 信息条 / 工具栏 / Toast / 帧条 / 解码指示（对话框转交 Dialog）
  icons.ts            # 线性 SVG 图标与工具栏定义
  types.ts            # IPC 类型与二进制信封解析（LicenseInfo 含权益位）
  ui.css              # 毛玻璃样式（设计 Token 按设计草图）
  components/dialog.ts # Modal 基类 + 授权对话框唯一主人（零件/显隐/行为缝）
  services/license.ts # 授权服务：权益答案源、激活/注销、购买跳转
  services/updater.ts # 自动更新检查
  utils/format.ts     # 纯函数：尺寸/位置/帧计数格式化
  lib/z-log.ts        # 前端统一日志转发
src-tauri/            # Rust 后端
  src/browse/         # 浏览模型：目录枚举、自然排序、跨文件夹导航（含门控）
  src/decode/         # 解码服务：RAW（预览快显 + rawler 全量）、TIFF、JXL、动画拆帧、EXIF 方向
  src/cache/          # LRU 解码缓存 + 文件夹首图队列（专业版）
  src/cache_policy.rs # 缓存策略：写过滤与预取过滤纯规则
  src/navigator.rs    # 深导航：导航 + 预取 + 代次协议唯一实现
  src/commands/       # Tauri IPC commands（含付费门控）
  src/util.rs         # 日志/时间小工具单一实现（basename_of / first_line_of / now）
  src/z_log.rs        # 统一日志：级别门槛、轮转修剪、脱敏、panic hook、导出诊断包
  src/analytics.rs    # 会话统计埋点：正常退出时上报聚合负载（不含路径）
  src/license.rs      # 专业版授权：设备指纹、验签、在线验证、激活
  src/main.rs         # 入口、启动参数、授权初始化
scripts/              # 图标 / 测试图片 / 版本号 / 发布 JSON 生成脚本
test-images/          # 生成的测试图库（A/B/C 跨文件夹验证）
```

## 项目状态与规划

见 `docs/开发阶段规划.md`。
