# 03. 解码分阶段计时 + 票 02 TIFF 数字反常验证

Status: resolved
Type: task
Priority: P1
Source: spec `.scratch/perf-flip-zoom/spec.md` 症状① R2 + 方法学存疑

## 问题

现有打点只有总耗时 `decode done {basename} in {ms}ms`（`decode/mod.rs:174-188`），
无法定位冷翻时间的构成。票 02 中 TIFF 12MP 无压缩冷翻 p50 ≈ 1.1s 反常：
无压缩 TIFF 解码理论近乎免费。假设：Windows Defender 对临时目录新写入噪声
文件的首次读取实时扫描（基准素材生成后立刻读取）。该假设影响所有基准数字
的可信度，也决定 issue 04 优化的靶点（解码 vs IO）。

## 方案

1. **分阶段计时**：`load_image_inner` 内按通道拆段（文件读取 / 引擎解码 /
   方向回正 / 降采样 / RGBA 转换 / 信封打包），debug 级单行输出
   （如 `decode stages {basename} read=12 engine=850 orient=3 resize=35 pack=18`）。
   实现放 decode 层一处（不散落到各通道），release 不落盘维持现状。
2. **bench 侧验证 Defender 假设**：`flip_latency_bench` 素材生成后先做一轮
   预热读（逐文件 read 全量丢弃）再计时；对比预热前后冷翻数字，差值即
   扫描开销估计。结论写入 `## Answer`。
3. 若确认 Defender 主导：bench 方法节加注（预热读为标准步骤），
   票 02 表格加脚注引用本票结论；真实用户数字以日志打点回收为准。

## 验收

- 分阶段打点落地且 `cargo test` 全绿；
- 预热前后的 bench 对照数据写入 `## Answer`，Defender 假设有明确结论；
- 票 02 表格按结论补脚注（不改历史数字，只注方法学）。

## Comments

- 2026-10-26 建档。本票是 issue 04 的前置输入：先知道时间花在哪，
  再决定「预览先显」怎么切。
- 2026-10-27 实施完成（agent）。

## Answer

**1. 分阶段计时已落地**：`decode/stage.rs` 单点机制（thread-local 收集，会话由
`load_image_inner` 控制；仅日志门槛为 debug 或 bench 显式 force 时收集，release
零开销不落盘）。debug 单行输出：

```
decode done big.tif in 75ms
decode stages big.tif read=10 engine=15 orient=0 resize=18 rgba=6 total=75
```

阶段语义：`read` 文件打开（TIFF 含 EXIF 方向解析）；`engine` 像素解码（JXL/RAW
引擎自带文件 IO，读取归入此段）；`orient`/`resize`/`rgba` 为 finalize_rgba 内
方向回正 / cap 降采样（全通道收口，含 JXL 逐帧封顶）/ RGBA 输出。`pack` 在
命令层且换线程执行，不进 decode 段行，bench 侧单独计时。可加性注意：JXL 通道
的逐帧 cap 发生在 engine 计时段内，该通道分段求和重复计入 resize（TIFF/RAW 严格
可加）。单测 `stage_timed_accumulates_and_begin_clears` 锁机制。

**2. Defender 假设验证（bench 预热读两趟对照，TIFF 4000×3000 ×8 无压缩，
2026-10-27 本机，release --no-default-features）**：

| 场景 | 未预热 冷p50 | 预热读后 冷p50 | 分段/张（未预热，ms） |
| --- | --- | --- | --- |
| level 0 免费（cold n=15） | 75.4ms | 78.5ms | read=10 engine=15 orient=0 resize=18 rgba=6 pack=7 |
| level 1 快速连翻（cold n=6） | 129.5ms | 140.2ms | read=17 engine=28 resize=30 rgba=8 pack=6 |
| level 2 快速连翻（cold n=5） | 119.6ms | 132.8ms | read=16 engine=25 resize=25 rgba=8 pack=10 |

预热读 275 MiB 仅 89ms（≈3 GB/s，纯页缓存速度）。

**结论：Defender 首读扫描假设在本机未复现**——未预热与预热后冷翻无显著差异
（75.4 → 78.5ms，噪声级），首读 read 段仅 10ms/张；且票 02 的 TIFF 1.1s 量级
本次完全未出现（同机同素材档位冷翻 p50 = 75ms）。历史反常数的失真来源未定
（当日机器状态/构建差异），已在本仓票 02 表格补方法学脚注（不改历史数字）。
预置预热读已固化为 bench 标准步骤（两趟对照结构），在真有 Defender 干扰的
机器上数字会自然分离；真实用户分布以日志 `decode stages` 打点回收为准。

**3. 对票 04 的输入（本票副产品）**：TIFF 冷解时间构成均匀（engine 15ms 仅占
75ms 的 20%，resize+rgba+pack+read 合计与引擎相当），TIFF 通道不存在「解码慢」
问题，无需预览先显（方案 C 成立）；冷翻慢的大头预期在 JXL engine 段（本机
no-default 构建跑不了 JXL bench 段，待 cmake 可用后以 `decode stages` 复核）。
