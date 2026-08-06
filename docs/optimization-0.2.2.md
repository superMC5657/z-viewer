# Image Viewer v0.2.3 性能优化报告

> 优化范围：前端幻灯片日志清理 + 后端 5 项性能优化（browse 导航 / cache / decode / 构建配置）
> 验证情况：`cargo test` 全部 32 个单测通过；`cargo check --release` 无警告；TypeScript 类型检查通过
> 报告日期：2026-08-06
> 状态：全部落地，测试通过

## 总结

本轮优化围绕「大图库浏览的算法复杂度」与「release 二进制零冗余」两个目标：

1. **browse 导航从 O(文件夹数) 每命令降为 O(1)** —— 全局位置/总数改为增量维护，不再每次状态读取全量累加
2. **目录排序消除 O(n log n) 次堆分配** —— `list_images` 与同级文件夹排序预计算文件名
3. **跨文件夹跳转免同步扫目录** —— `FolderFirstCache` 随值保存首图路径，`promote` 不再在 AppState 锁内 `read_dir`
4. **长 GIF 峰值内存从「全部帧」降到「~2 帧」** —— 动画拆帧边解码边编码、逐帧释放
5. **解码热路径 release 单独 -O3** —— `image`/`rawler` 提速，其余 crate 保持 -Os 保体积
6. **前端删掉两个零引用的死 getter**（`slideshow.ts`），顺带消除 release 下 `dev_log` 产生的未使用变量警告
7. **`open()` 免 N 次 `canonicalize` 系统调用** —— 改用文件夹名定位当前文件夹

---

## 优化项明细

### ① `state_from_inner` 增量维护 `global_index` / `global_total`（O(n) → O(1)）

- 位置：`src-tauri/src/browse/{mod,scan,nav,state}.rs`
- **改动前**：`state_from_inner` 每次计算 `BrowseState` 都遍历全部文件夹累加已填充图片数。每个导航命令（next/prev/jump）都会触发一次状态读取，同级文件夹几百上千个时整体退化为 O(n²)。
- **改动后**：
  - `InnerData` 新增 `global_index` / `global_total` 两个增量字段
  - `open()` 初始化（仅当前文件夹已填充 → 总数=当前图片数，位置=当前索引）
  - `background_scan` 填充某文件夹时 `+= len`（当前文件夹之前的填充还会把 `global_index` 前移）
  - 导航时同文件夹 ±1；跨文件夹用 `move_to_folder` 按 `Σ[old_fi..new_fi)` 已填充数校正（相邻跳转 O(1)，First/Last 长距离跳转 O(range)，低频可接受）
  - 空文件夹压缩后一次性重算（一次性的 O(n)，此后为增量基准）
- **收益**：状态读取恒 O(1)；图片级/相邻文件夹导航 O(1)；大图库整体从 O(n²) 降为 O(n)。

### ② `list_images` 排序预计算文件名，消除比较器内分配

- 位置：`src-tauri/src/browse/scan.rs`（`list_images` 与 `open()` 的同级文件夹排序）
- **改动前**：`sort_by` 比较器内对每个比较双方都执行 `file_name().to_string_lossy().to_string()` —— 每次比较分配两个 String，O(n log n) 次堆分配（5000 张图 ≈ 6 万次分配）。
- **改动后**：先一次性构建 `Vec<(name, path)>`（每条目一次分配），排序后丢弃名字取路径。降为 O(n) 次分配。
- **收益**：大文件夹（数千张图）首屏打开与后台扫描明显提速。

### ③ `FolderFirstCache` 随值保存首图路径，`promote` 免同步扫目录

- 位置：`src-tauri/src/cache/{mod,folder_first}.rs`、`src-tauri/src/commands/prefetch.rs`
- **改动前**：`promote_folder_first` 在持有全局 `AppState` 锁的导航路径上调用 `first_image_of` → `list_images`（`read_dir` + 排序）重新定位首图。锁内同步文件系统枚举会阻塞所有导航命令（即旧报告 P3-1 指出的模式，当时只修了 `get_context`/`prefetch_folder_firsts`，此处是漏网之鱼）。
- **改动后**：
  - `LruQueue` 泛型化为 `LruQueue<V>`（`DecodeCache` 用 `Arc<LoadResult>`，`FolderFirstCache` 用 `Arc<FolderFirst>`）
  - `FolderFirst` 结构体同时携带 `path`（首图路径）与 `result`（解码结果）
  - `prefetch_folder_firsts` 后台解出首图时一并写入路径
  - `promote_folder_first` 直接 `cache.put(first.path.clone(), first.result.clone())`，零目录枚举
- **收益**：跨文件夹跳转导航路径上不再有同步文件系统操作；同时 `LruQueue` 泛型化消除了两套重复的 LRU 实现。

### ④ 动画拆帧边解码边编码，逐帧释放（长 GIF 峰值内存 ↓）

- 位置：`src-tauri/src/decode/animation.rs`（`collect_frames`）
- **改动前**：先把全部帧解码进 `Vec<(RgbaImage, u32)>` 驻留内存，再逐个编码 PNG。长 GIF（如 200 帧 1024×768）峰值内存 ≈ 所有解码帧 + 单个 PNG 缓冲，可达数百 MB。
- **改动后**：前两帧仅解码（判定是否多帧，`< 2` 直接降级 asset 不编码，保留原「避免无谓编码」语义）；确认多帧后边解码边编码、逐帧释放。
- **收益**：峰值内存从「全部帧」降到「~2 帧」；`frames[0].delay_ms` 等行为语义不变（单测覆盖）。

### ⑤ release 对解码热路径单独 `opt-level = 3`

- 位置：`src-tauri/Cargo.toml`
- **改动**：`image` / `rawler` 两个纯 CPU 解码热路径 crate 单独提为 `-O3`，其余 crate 维持 `-Os` 保体积。
- **收益**：JPEG/RAW 解码、demosaic 通常比 `-Os` 快 10-30%；体积代价仅限这两个 crate。

### ⑥ 前端删除死代码 + release 日志门控

- `src/slideshow.ts`：删除全项目零引用的 `get isRunning()` / `get interval()` 两个 getter（含两条永远打不出的日志）。
- `src-tauri/src/commands/mod.rs`：`next_image` 的 `dev_log` 调用点整体加 `#[cfg(debug_assertions)]`，并把 `use crate::dev_log` 一同门控 —— release 下连 `if let` 绑定一起剥离，消除「dev_log 空展开 → 未使用变量」警告，release 构建零警告。

### ⑦ `open()` 定位当前文件夹免去 N 次 `canonicalize` 系统调用

- 位置：`src-tauri/src/browse/scan.rs`（`open()`）
- **改动前**：构建 `folders` 时循环内对每个兄弟目录执行 `canonical(d)`（`std::fs::canonicalize`，一次 fs 系统调用）来匹配当前文件夹。N 个同级目录 = N 次 syscall，全部阻塞在启动/打开路径上。
- **改动后**：同级目录名唯一，改为比较 `file_name`（OsStr 相等比较，零分配零 syscall），仅保留一次 `canonical(parent)`。
- **收益**：启动打开（含命令行传参、双击图片）在兄弟目录多时明显提速；`open_case_insensitive` 等大小写测试覆盖了行为一致性。

---

## 验证结果

| 项目 | 结果 |
|------|------|
| `cargo test`（browse/cache/commands/decode） | ✅ 32 passed, 0 failed |
| `cargo check --release` | ✅ 无警告 |
| `cargo check --all-targets` | ✅ 无错误 |
| TypeScript `tsc --noEmit` | ✅ exit 0 |

覆盖到关键回归点：`async_open_large_library_is_immediate`（增量计数初始化与后台填充）、`global_index_accumulates`（跨文件夹累计）、`prev_and_first_boundary`（反向跨文件夹）、`nav_during_scan_never_panics`（扫描期导航竞态）、`gif_animation_frames`（流式拆帧语义）。

## 取舍与遗留

- `move_to_folder` 的 First/Last 长距离跳转仍是 O(range)，但这是低频用户动作且与旧实现同阶，不构成回归。
- 动画拆帧的 `image::Frames` 迭代器为单次消费，为维持「<2 帧判定静态」需要先解两帧再开始编码，故峰值是「~2 帧」而非严格 1 帧。
- 未做改动：`dev_log!` 宏的 cfg 方案本身已是 release 零成本的最优解（编译期剥离，连调用点参数都不求值），保持原样。
