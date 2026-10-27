# 翻页延迟与放大清晰度 — 性能改进 spec（2026-10-26）

来源：用户反馈两个严重性能问题——①切换图片太慢；②放大图片相较于 JpegView 很糊。
本 spec 是 `.scratch/pm-followup-20261024/issues/03` Answer 中预留的
「解码提速/预览先显」后续立项，并叠加两个已定位的放大清晰度 bug。

## 症状与根因对照

### 症状② 放大比 JpegView 糊 —— 根因已锁定（两个 bug，一个质量项）

**BUG-A（P0）深放大真像素升级把升级结果直接丢弃。**
`src/presenter.ts:359-362`：`maybeFetchNative` 收到 `original=true` 的返回后，
遇到 `!header.capped` 就早退并标记「无需升级」。但该请求走
`load_image_original`（cap = ZOOM_MAX_DIM = 8192，`decode/mod.rs:273`），
`capped=false` 的真实含义是「原图 ≤ 8192」，**不是**「原图 ≤ 显示帽 3840」。
早退分支唯一可达的区间恰好是 **3840 < 原图长边 ≤ 8192**——即 24MP 主流全画幅
（6000×4000）、45MP（8192×5464）等全部日常素材。此区间内后端已把全分辨率
位图递到前端，前端却扔掉，永远继续显示 3840 Bilinear 降采样图。
佐证：`decode/tests.rs:437`（`original_decode_bypasses_display_cap`）锁定的后端
契约正是「4000×2800 original 解码返回 4000 宽 + capped=false」——被丢弃的就是
这份真像素数据；票 07 的发版手验项（放大 TIFF/JXL/RAW 至 1:1 看细节）正是本
场景。该分支自实现提交 `63f5996` 起即如此。

**BUG-B（P1）「实际大小」模式不触发深放大取数。**
`src/presenter.ts:345`：触发阈值为 `currentScale <= 1 return`（严格大于才取）。
进入 actual 模式时 `currentScale` 恰好 = 1，永不触发。用户语义里「实际大小」
应看到原图 1:1 像素，当前对 >3840 的源显示的是 3840 位图的 1:1（视觉上就是
对插值图的 1:1，与 JpegView 的原图 1:1 对比明显偏糊）。

**质量项（P1）显示帽降采样用 Bilinear。**
`decode/mod.rs:311-316`（`cap_dimensions`）与 jxl 通道 `cap_rgba_frame` 用
`ResizeAlg::Convolution(Bilinear)`（tent 核）做 6000→3840 一类降采样，比
JpegView 的高质量重采样（WIC Fant/Lanczos 类）明显偏软。fast_image_resize 6.1
自带 CatmullRom/Lanczos3（SIMD + rayon），换滤波是低成本改进。

排除项（查过、不是原因）：asset 通道（jpg/png/webp）全分辨率直显无帽；
CSS transform 渲染路径正常（`ui.css` image-rendering: auto，双缓冲瞬切）；
`viewer.showUpgrade` 的倍率回正数学正确；canvas 通道绘制正常。

### 症状① 切换图片慢 —— 多因，量化在案（票 02，2026-10-24 本机记录）

| 因素 | 证据 | 量级 |
| --- | --- | --- |
| R1 免费档零预取（产品决策，票 03 已裁决维持） | `commands/mod.rs:291-299`（caching=false）、`431-433`（get_context 免费返回 []）→ 每翻必冷解 | JXL 24MP 冷翻 p50 ≈ 1.4s（合成最坏素材） |
| R2 解码流水线本身慢：全分辨率解码→方向回正→降采样→RGBA | 票 02 结论 2：专业档命中率仅 53–80%，冷解 > 翻页间隔，预取追不上 | 冷解 p50 1.1–1.4s（真实照片预期更快，仍在感知线附近） |
| R3 预取窗口小 + 与实时解码争 CPU | `navigator.rs`（专1=前1后1、专2=前1后3）；`decode_cache.rs:16` PREFETCH_MAX=2，预取与实时解码同池 | 命中 10–13ms（缓存住=秒开），冷翻占一半以上 |
| R4 RGBA 信封传输链：33MB 级多次拷贝 | `pack_envelope` → `Response::clone` → IPC → `ImageData` → `createImageBitmap` → `drawImage` | 数十 ms 级（次要） |
| R5 大文件夹导航在 Condvar 上等待后台扫描 | `browse/state.rs` `wait_filled`（`navigate` 命令同步等待填充） | 环境相关（慢盘/网络盘可达秒级） |

**基准方法学存疑（待证）**：票 02 中 TIFF 12MP 无压缩冷翻 p50 也有 1.1s——无压缩
TIFF 解码理论上近乎免费，此数字反常。假设：Windows Defender 对临时目录新写入
36MB 噪声文件的首次读取实时扫描。需分阶段计时（issue 03）区分解码/IO/传输占比，
真实用户数据以 `decode done {basename} in {ms}ms` 打点回收。
注：本机当前缺 cmake，`flip_latency_bench`（默认 feature 含 vendored libjxl）
无法在本地复跑；数据采用票 02 记录值。

## 改造目标

1. **放大清晰度追平 JpegView**：>3840 源在放大/实际大小下显示真像素（≤8192），
   显示帽降采样换高质量滤波。
2. **冷翻压进感知线**：IPC 通道（JXL/TIFF/RAW）冷翻 p50 < 300ms（真实照片素材，
   非合成最坏）；先用预览先显把「看到图」时间与「全量清晰」时间解耦。
3. 不动票 03 已裁决的免费/专业分层（免费版维持零预取）。

## Issues

| # | 内容 | 优先级 | 状态 |
| --- | --- | --- | --- |
| 01 | 修复深放大真像素升级丢弃（BUG-A + BUG-B，含 TS 测试基建与回归测试） | P0 | resolved |
| 02 | 显示帽降采样 Bilinear → CatmullRom | P1 | resolved |
| 03 | 解码分阶段计时（解码/回正/降采样/打包/IO 拆分）+ Defender 假设验证 | P1 | resolved |
| 04 | JXL/静态通道「预览先显」spike（libjxl 渐进 flush / 两段式交付评估） | P1 | ready-for-agent |
| 05 | 预取与实时解码资源让路（demand in-flight 时预取降并发/暂停） | P2 | resolved |
| 06 | RGBA 信封传输降载评估（决策票：RGBA vs JPEG 交付 vs 半尺寸） | P2 | ready-for-human |
| 07 | 大文件夹导航 Condvar 等待优化（先量化再改） | P2 | needs-info |

依赖关系：01/02 独立可并行；04 的方案选择依赖 03 的计时数据；05/06 低优先级。

## 非目标

- >8192 长边全景图的 tile 化（票 07 已列为后续立项，本 spec 不重复）。
- 免费版预取分层调整（票 03 已裁决维持 A）。
- 跨文件夹首图队列（队列 A）行为变更。
