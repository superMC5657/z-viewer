# 02. 显示帽降采样滤波升级：Bilinear → CatmullRom

Status: ready-for-agent
Type: task
Priority: P1
Source: spec `.scratch/perf-flip-zoom/spec.md` 症状② 质量项

## 问题

`src-tauri/src/decode/mod.rs:311-316` `cap_dimensions` 用
`ResizeAlg::Convolution(FilterType::Bilinear)`（tent 核）做显示帽降采样
（如 6000→3840、8192→3840）。Bilinear 降采样比 JpegView 的高质量重采样
（WIC Fant/Lanczos 类）明显偏软——适配窗口态就已损失细节，放大后愈发明显。
jxl 通道 `cap_rgba_frame` 复用 `cap_dimensions`，同受影响。

## 方案

- `cap_dimensions` 滤波换 `fast_image_resize::ResizeAlg::Convolution(FilterType::CatmullRom)`
  （6.1 内置，SIMD + rayon；Lanczos3 更锐但有振铃，CatmullRom 是锐度/伪影折中，
  与 JpegView 观感最接近）。`use_alpha(false)` 语义不变。
- 用 `flip_latency_bench` 复跑记录降采样耗时增量（ CatmullRom 预计为 Bilinear
  的 2–3 倍，但绝对值在几十 ms 级，对冷翻占比小）；若增量异常大再回退权衡。
- 注释同步：`decode/mod.rs:291-293`「Triangle/Bilinear 卷积」描述更新，
  说明与 image::resize Triangle 的历史等价关系不再成立（有意变更）。

## 验收

- `cargo test` 全绿（`original_decode_bypasses_display_cap` 不断言像素，不受影响）；
- bench 记录前后耗时对照写入本票 `## Answer`；
- 手验：>3840 源适配窗口态截图对比（换滤波前后）可辨锐度提升、无振铃伪影。

## Comments

- 2026-10-26 建档。
