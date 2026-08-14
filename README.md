# Image Viewer

基于 Tauri 2.x 的精美快速看图软件。深色毛玻璃 UI，核心特色是**跨同级文件夹无缝浏览**与**RAW/动画格式支持**。

> 文档：`docs/需求报告与技术方案.md`（需求与技术方案）、`docs/UI设计草图.md`（界面唯一依据）、`docs/开发阶段规划.md`（里程碑追踪）、`docs/埋点协议-analytics.md`（会话统计埋点协议）

## 技术栈

- **应用框架**：Tauri 2.x（Rust 后端 + WebView2 前端）
- **前端**：Vanilla TypeScript + Vite（无框架运行时开销）
- **包管理**：pnpm
- **图片解码**：rawler（RAW：CR2/CR3/NEF/ARW/DNG 等）+ image crate（GIF/APNG/WebP 动画拆帧）

## 功能

- 跨同级文件夹无缝浏览（natord 自然排序，与资源管理器一致）— **专业版**
- 常见格式：JPG / PNG / GIF / WebP / BMP / ICO / SVG
- 相机 RAW：CR2 / CR3 / NEF / ARW / DNG 等（rawler 解码 + demosaic + 降采样显示）
- 动画格式：GIF / APNG / 动态 WebP 逐帧控制（播放/暂停/上一帧/下一帧）
- 沉浸模式（F）、窗口置顶（T）、缩放/旋转/翻转、1:1/适应窗口
- 幻灯片播放（空格/▶，间隔 2s/5s/10s，自动跨文件夹，播放完自动停止）
- 预加载缓存：相邻图片后台预取，RAW/动画 LRU 缓存（切换 ≤50ms）— **专业版**
- 异步浏览：打开图片立即显示（只枚举当前文件夹），同级文件夹后台枚举，
  完成后自动刷新位置计数（加载中显示 "3/…"）

> **专业版**（跨文件夹浏览 + 预取缓存）通过 soft-candy 在线激活码解锁，
> 免费版仅单文件夹浏览且不缓存。授权协议见 soft-candy `docs/api.md` §4.2/4.3。

## 专业版解锁

应用内点「缓存」或「文件夹跳转」按钮 → 解锁对话框 → 输入激活码。

- 激活码由 soft-candy 服务端绑定设备，签发 EdDSA JWT
- JWT claims：`iss/aud/sub/code/app/level/levelLabel/deviceId/features/iat/exp`
- 客户端内置当前产品对应等级的公钥，离线验签可用；启动时联网续验换新 JWT
- 激活/续验地址、等级公钥在 `src-tauri/tauri.conf.json` → `plugins.store` 配置
- 等级公钥支持 raw 32 字节 base64、完整 SPKI DER base64 或 PEM，客户端会自动解包

## 开发

```bash
pnpm install          # 安装前端依赖
pnpm gen:test-images  # （可选）生成 test-images/ 测试图库
pnpm tauri dev        # 开发模式（前端热更新 + Rust 自动重编译）
```

**打开图片**：

- 命令行传参：`pnpm tauri dev -- test-images/A/10.png`
- 拖拽图片 / 文件夹到窗口任意位置
- 正式安装后（M5）支持双击图片直接打开

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

## 构建

```bash
pnpm tauri build -- --no-bundle   # 仅产出可执行文件（快速验证）
pnpm tauri build                  # 完整打包（NSIS 安装包）
```

## 目录结构

```
src/                  # 前端代码
  main.ts             # 入口、状态机、IPC
  viewer.ts           # 渲染与变换（缩放/旋转/翻转，rAF 合并 60fps）
  input.ts            # 快捷键 / 滚轮
  window-state.ts     # 沉浸模式 / 窗口置顶控制
  slideshow.ts        # 幻灯片播放器（间隔/边界/计时）
  ui.ts               # 信息条 / 工具栏 / Toast / 浮层 / 解锁对话框
  icons.ts            # 线性 SVG 图标与工具栏定义
  ui.css              # 毛玻璃样式（设计 Token 按设计草图）
src-tauri/            # Rust 后端
  src/browse/         # 浏览模型：目录枚举、自然排序、跨文件夹导航（含门控）
  src/decode/         # 解码服务：RAW（rawler）+ 动画拆帧（image crate）+ 通道分发
  src/cache/          # LRU 解码缓存 + 文件夹首图队列（专业版）
  src/commands/       # Tauri IPC commands（含付费门控）
  src/license.rs      # 专业版授权：设备指纹、验签、在线验证、激活
  src/main.rs         # 入口、启动参数、授权初始化
backend-mock/         # 旧授权参考实现，已废弃（soft-candy 为当前授权服务）
scripts/              # 图标 / 测试图片生成脚本
test-images/          # 生成的测试图库（A/B/C 跨文件夹验证）
```

## 里程碑进度

见 `docs/开发阶段规划.md`。
