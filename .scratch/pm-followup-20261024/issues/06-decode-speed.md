# 06. 解码提速:把冷翻 p50 压进感知线

Status: resolved
Source: 票 02/票 03 的共同结论——瓶颈在解码流水线(冷翻 p50 1.1–1.4s),不在缓存结构
Type: task

## 问题

票 02 基准:JXL 24MP / TIFF 12MP 冷翻 p50 ≈ 1.1–1.4s,远超 ~300ms 感知线;
免费档无解,专业档预取也追不上(命中率 53–80%)。命中后 10–13ms 秒开——
优化对象是「第一次解码」全链路:读盘 → 引擎解码 → 方向回正 → 3840 降采样 → RGBA → 信封。

## 方法

1. 分段剖析(`decode_component_profile`,#[ignore] release 手动跑)定位大头;
2. 按数据下药,候选:
   - JXL 引擎解码占比高 → 提高引擎线程钳制(现 ≤8)/ FFI 加 FlushImage 做 DC 级预览先显
   - 收尾(缩放)占比高 → 并行缩放或大倍率降采样换滤波策略
   - 信封/读盘占比高 → 拷贝消除
3. 复跑 flip_latency_bench 对比前后,结果写回本票。

## 验收

- 剖析表 + 优化前后对比表写入 `## Answer`;
- cargo test 全绿、clippy 干净;行为不变(输出像素通道一致)。

## Answer

2026-10-24 完成。分段剖析(`decode_component_profile`)推翻了「解码引擎是大头」的假设:

| 分段 | JXL 24MP 前→后 | TIFF 12MP 前→后 |
| --- | --- | --- |
| 读盘 | 2.9 → 4.2ms | 10.8 → 12.5ms |
| 引擎/解码器解码 | 325 → 298ms | 16.7 → 18.8ms |
| 收尾(方向+缩放+RGBA) | **1391 → 77ms** | **1256 → 39ms** |
| 信封打包 | 6.6 → 8.0ms | — |

真凶是 `finalize_rgba` 里 image crate 的单线程 Triangle 缩放(占冷翻 80–97%)。
换 `fast_image_resize`(SIMD + rayon 并行;Bilinear 与原 Triangle 同一 tent 核,
非预乘 alpha 行为一致):收尾段 JXL 18×、TIFF 32× 提速,输出仍为 RGBA8 直显通道。

冷翻 p50(免费档,`flip_latency_bench` 复跑):

| | 优化前 | 优化后 |
| --- | --- | --- |
| JXL 24MP | 1381ms | **344ms** |
| TIFF 12MP | 1113ms | **74ms** |

专业档命中率同步改善(预取追得上了):JXL 专1 正常节奏 53%→87%,TIFF 60%→93%。

遗留旋钮(本票不做,数据已备):

- JXL 引擎线程钳制 `MAX_THREADS = 8`(本机 16 核),放开可再砍引擎解码约一半;
  代价是后台预取的 CPU 争用放大,需结合 `PREFETCH_MAX` 一起调。
- JXL 免费档 344ms 仍略高于 ~300ms 感知线(合成噪声素材为保守上界,真实照片更快);
  DC 级预览先显需给 FFI 加 FlushImage/Progressive 绑定,值得单独立项。

## Comments

- 2026-10-24 建档并认领;剖析探针随本票落地。
- 2026-10-24 完成(见 Answer);CI clippy 顺带升为 --all-targets(测试代码纳入 lint)。
