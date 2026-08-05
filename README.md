# Image Viewer

基于 Tauri 2.x 的精美快速看图软件。深色毛玻璃 UI，核心特色是**跨同级文件夹无缝浏览**。

> 文档：`docs/需求报告与技术方案.md`（需求与技术方案）、`docs/UI设计草图.md`（界面唯一依据）、`docs/开发阶段规划.md`（里程碑追踪）

## 技术栈

- **应用框架**：Tauri 2.x（Rust 后端 + WebView2 前端）
- **前端**：Vanilla TypeScript + Vite（无框架运行时开销）
- **包管理**：pnpm

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

## 快捷键（M1 已实现）

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
  viewer.ts           # 渲染与变换（缩放/旋转/翻转）
  input.ts            # 快捷键 / 滚轮
  ui.ts               # 信息条 / 工具栏 / Toast / 浮层
  icons.ts            # 线性 SVG 图标与工具栏定义
  ui.css              # 毛玻璃样式（设计 Token 按设计草图）
src-tauri/            # Rust 后端
  src/browse.rs       # 浏览模型：目录枚举、自然排序、跨文件夹导航
  src/commands.rs     # Tauri IPC commands
  src/main.rs         # 入口、启动参数
scripts/              # 图标 / 测试图片生成脚本
test-images/          # 生成的测试图库（A/B/C 跨文件夹验证）
```

## 里程碑进度

见 `docs/开发阶段规划.md`。
