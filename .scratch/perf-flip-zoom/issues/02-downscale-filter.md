# 02. 显示帽降采样滤波升级：Bilinear → CatmullRom

Status: resolved
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

## Answer

**改动**（`decode/mod.rs cap_dimensions`）：`ResizeAlg::Convolution(Bilinear)` →
`Convolution(CatmullRom)`，`use_alpha(false)` 语义不变；jxl 通道 `cap_rgba_frame`
复用 `cap_dimensions` 一并生效。注释同步：滤波历史 Triangle→Bilinear（等价迁移，
票 06）→CatmullRom（票 02，有意变更）。

**bench 耗时对照**（flip_latency_bench，TIFF 4000×3000 ×8 无压缩，release
--no-default-features，分段计时见票 03；各档取未预热趟，level 0 为 cold n=15
统计有效行）：

| 场景 | Bilinear 冷p50 | CatmullRom 冷p50 | Bilinear resize/张 | CatmullRom resize/张 |
| --- | --- | --- | --- | --- |
| level 0 免费（cold n=15） | 75.4ms | 75.9ms | 18ms | 18ms |
| level 1 快速连翻（n=6） | 129.5ms | 139.6ms | 30ms | 33ms |
| level 2 快速连翻（n=4-5） | 119.6ms | 122.1ms | 25ms | 26ms |

**结论**：4000→3840（1.04×，小比例降采样）下 CatmullRom 与 Bilinear 无可测差异
（resize 段 18→18ms，总冷翻 75.4→75.9ms，均在噪声内）——预期「2–3 倍增量」在
此尺寸不成立；更陡的 6000→3840（24MP JXL 档）增量会更大但量级仍应远小于引擎
解码，且本机 no-default 构建无法跑 JXL bench 段（cmake 缺失），留 `decode stages`
日志打点回收。锐度收益以换滤波的算法性质（4-tap cubic vs 2-tap tent）为据。

**验收核对**：`cargo test` 全绿（--no-default-features，96 passed；默认 feature
构建待 cmake）；bench 对照已录；「>3840 源适配窗口态截图对比可辨锐度提升、无
振铃伪影」为发版手验项（与票 01 的 1:1 细节对比同场景执行）。
