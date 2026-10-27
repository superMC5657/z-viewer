# 04. spike：JXL/TIFF 通道「预览先显」——冷翻感知线优化

Status: ready-for-agent
Type: research
Priority: P1
Blocked by: 03
Source: spec `.scratch/perf-flip-zoom/spec.md` 症状① R2；票 03 Answer 点名的后续立项

## 问题

RAW 已有「内嵌预览快显 → 后台全量」两段式（`raw.rs` + `scheduleUpgrade`），
JXL/TIFF 没有：冷翻必须等全分辨率解码+降采样完成才上屏（票 02 合成最坏
p50 1.1–1.4s；真实照片预期数百 ms，仍在感知线附近）。

## 待评估方案（spike 范围）

- **A. libjxl 渐进 flush**：`JxlDecoderFlushImage`（vendored 0.12 FFI 已有绑定层
  可扩）在 ProcessInput 中途取当前渐进态（VarDCT 码流有 1/8、1/2 DC 档），
  先上屏低清再续解全量。注意：modular/无损码流无 DC 档，flush 结果按码流
  而异——需测真实照片与 bench 噪声素材两类码流。
- **B. 两段式解码**：先解 DC 档（若 FFI 可指定降采样输出）/或先解低分辨率
  副本（JXL 若内嵌 preview frame），超时阈值后全量替换；复用 RAW 的
  scheduleUpgrade/showUpgrade 机制与代次守卫。
- **C. TIFF 快路径**：无压缩/常见压缩 TIFF 解码本身应快（结合 issue 03 计时
  确认真实瓶颈在解码还是 IO），可能只需 IO 侧优化而非预览先显。

## 决策输入

- issue 03 的分阶段计时（时间在引擎解码还是 IO）；
- 各方案实现复杂度（FFI 改动量、代次守卫/双缓冲是否复用现状即可）；
- 对 `capped`/`isPreview` 信封语义的影响（是否新增模式，前端 parseLoadEnvelope
  同步面）。

## 产出

本票 `## Answer` 写明：选定方案、原型分支或 PoC 提交、冷翻「首屏时间」
对比数据（bench + 至少 3 张真实 JXL 照片）、遗留风险。可行则拆实施票。

## Comments

- 2026-10-26 建档。
