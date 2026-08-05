# Image Viewer v0.1.0 代码审查报告

> 审查范围：核心浏览 / 缓存 / 解码流程（TypeScript 前端 + Rust 后端）
> 验证情况：TypeScript 类型检查通过；Rust 全部 30 个单测通过
> 审查日期：2026-08-06
> 状态：存在 2 个 P2 正确性缺陷，建议合并前修复；其余为性能与健壮性改进项

## 总结

核心浏览 / 缓存 / 解码流程可运行，但存在两个会实际影响使用的正确性缺陷：

1. 慢图加载超过 2.5s 时永久黑屏（`loadStatic` 超时兜底清空 pending，迟到的 load 事件被丢弃）
2. 后台扫描期间导航进空文件夹会得到空状态，且该空文件夹不再被移除

建议在合并前修复以上两个 P2 问题，其余为性能与健壮性改进项。

---

## P2 正确性缺陷

### P2-1 `loadStatic` 超时后丢弃迟到的 load 事件，慢图永久黑屏

- 位置：`src/viewer.ts:123-129`（`loadStatic` 超时兜底）、`src/viewer.ts:377-380`（`handleLoad`）
- 严重度：P2（会影响实际使用）

**问题描述**

`viewer.loadStatic` 的 2.5s 超时兜底触发时把 `this.pending` 置为 `null` 并 resolve，但之后图片真正加载完成时 `handleLoad` 因 `!this.pending` 直接 return，`handleDecoded` 永远不会执行：

- `loaded` 保持 `false`
- `visible` class 不会被加上
- 图片以 `opacity: 0` 永久停在黑屏

任何超过 2.5s 才完成加载的图片（大 JPEG/BMP、慢盘或网络盘、WebView2 首次解码大图）都会静默空白，只能再翻页离开后回到同 URL 走 `decode()` 分支才恢复。

**建议修复**

超时应只 resolve 而不清 pending，或让 `handleLoad` 以 `seq` 而非 `pending` 判定是否丢弃。

---

### P2-2 扫描期间导航进空兄弟文件夹产生空状态且文件夹不再移除

- 位置：`src-tauri/src/browse/nav.rs:17-20`（`next()` 跨文件夹分支）、`src-tauri/src/browse/scan.rs:161-175`（空文件夹压缩）
- 严重度：P2（会影响实际使用）

**问题描述**

后台枚举完成前，在 A 末尾按 →/PgDn（或跳到 Last）进入尚未扫描且实际为空的兄弟文件夹 B 时：

- `next()` / `jump_folder` 不检查 B 是否为空就返回 `Nav::Ok`
- `state()` 等待扫描完成后返回 `path=""` 的 `BrowseState`
- 前端对空路径 `loadStatic(convertFileSrc(""))` 必然失败，弹「无法显示图片」并黑屏

同时 `scan.rs` 压缩空文件夹时因 `i != old_index` 保留当前文件夹，空 B 会永久留在 `folders` 列表，`folder_total` 计数虚高，之后文件夹跳转仍可能再次落到空文件夹。

**建议修复**

跨文件夹导航时跳过已填充的空文件夹，或压缩时允许移除当前空文件夹并指向相邻非空文件夹。

---

## P3 性能与健壮性改进项

### P3-1 导航/取上下文时在锁内同步枚举整个邻居目录

- 位置：`src-tauri/src/commands/prefetch.rs:53-59`、`src-tauri/src/commands/mod.rs:249-278`（`get_context`）
- 严重度：P3

**问题描述**

`prefetch_folder_firsts` 和 `get_context` 在持有 AppState（以及 settings）锁的情况下同步调用 `BrowseModel::first_image_of` → `list_images`，对每个邻居文件夹做整目录 `read_dir` 只为取第一张图。邻居目录文件数多（数万文件）时，每次导航或取上下文都会在命令线程上阻塞，期间所有需要 state 锁的命令（next/prev/jump_folder/open_path/get_context）全部卡住。

**建议修复**

把首图查找（至少目录枚举）移入 `spawn_blocking`，或复用后台扫描的填充结果，避免导航被目录枚举拖慢。

---

### P3-2 缓存关闭后 `get_context` 仍返回首图路径，前端继续预热

- 位置：`src-tauri/src/commands/mod.rs:268-278`（`get_context`）
- 严重度：P3

**问题描述**

缓存等级 0（关闭）时 Rust 侧 `prefetch_context` / `prefetch_folder_firsts` 都因 `is_enabled()` / `neighbor_window()` 提前返回，但 `get_context` 没有同样的检查：只要 `folder_first_depth > 0`（默认 1），它仍把相邻文件夹首图路径返回给前端，而前端 `refreshContext` 无条件调用 `prefetch.warm` 通过 asset 协议预加载这些图片。用户点击「预取缓存已关闭」后 WebView 仍在后台解码邻居图片，与「0=关闭（不缓存不预取）」的语义矛盾。

**建议修复**

`get_context` 应在 `cache_level == 0` 时直接返回空列表。

---

### P3-3 `togglePin` 缺少防重入守卫，双击无法取消置顶

- 位置：`src/window-state.ts:60-69`（`togglePin`）
- 严重度：P3

**问题描述**

`togglePin` 没有 `toggleImmersive` 那样的 in-flight 防重入：连续快速点击两次置顶按钮时，两次都在第一次 `await setAlwaysOnTop` 返回前读到 `pinned === false`，都向 true 切换，最终状态仍是置顶，第二次点击无法取消。

**建议修复**

`toggleImmersive` 已用 `toggling` 标志处理同类竞态，`togglePin` 应沿用同一模式。

---

### P3-4 `release.sh` `git add -A` 会把未忽略的 `.reasonix` 会话日志提交进公开仓库

- 位置：`scripts/release.sh`（原第 74 行）
- 严重度：P3

**问题描述**

`scripts/release.sh` 使用 `git add -A` 提交全部改动，而仓库当前存在未跟踪的 `.reasonix/`（开发会话日志目录，未加入 `.gitignore`），脚本还会通过 `gh repo create --public` 把仓库设为公开。下次发版会把会话日志一并提交并推送到公开仓库，存在信息泄露风险。

**修复状态（已核对）**

该脚本已在提交 `3909452`（chore(release): 移除本地构建上传脚本，发版统一走 CI）中被删除，本地 `git add -A` 泄露途径已消除。但 `.reasonix/` 仍为未跟踪目录且未加入 `.gitignore`（`git status` 显示 `?? .reasonix/`），仍有被误提交的风险，建议在 `.gitignore` 中加入 `.reasonix/`。

---

## 修复建议清单

| 编号 | 严重度 | 位置 | 问题 | 建议 |
|------|--------|------|------|------|
| P2-1 | P2 | `src/viewer.ts:123-129` | 超时后迟到 load 事件被丢弃，慢图永久黑屏 | 超时只 resolve 不清 pending，或按 seq 判定 |
| P2-2 | P2 | `src-tauri/src/browse/nav.rs:17-20` | 导航进未扫描空文件夹 → 空状态且不再被移除 | 跨文件夹导航跳过空文件夹，或压缩允许移除当前空文件夹 |
| P3-1 | P3 | `src-tauri/src/commands/prefetch.rs:53-59` | 锁内同步枚举邻居目录，阻塞全部导航命令 | 首图查找移入 `spawn_blocking` 或复用扫描结果 |
| P3-2 | P3 | `src-tauri/src/commands/mod.rs:268-278` | 缓存关闭后仍返回首图路径，前端继续预热 | `cache_level==0` 时 `get_context` 返回空列表 |
| P3-3 | P3 | `src/window-state.ts:60-69` | `togglePin` 无防重入，双击无法取消置顶 | 沿用 `toggleImmersive` 的 `toggling` 模式 |
| P3-4 | P3 | `.gitignore` | `.reasonix/` 未忽略（release.sh 已删除，残留风险） | `.gitignore` 加入 `.reasonix/` |
