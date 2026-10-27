# 03. 解码分阶段计时 + 票 02 TIFF 数字反常验证

Status: ready-for-agent
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
