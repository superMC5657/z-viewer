# ADR 0001：JXL 解码主引擎采用 vendored libjxl（clang-cl 构建），jxl-oxide 回退

日期：2026-10-24
状态：已采纳

## 背景

JXL 通道最初用 jxl-oxide（纯 Rust）解码。用户反馈大图解码过慢。基准
（3000×2000 确定性纹理，`decode::tests::jxl_engine_bench`）：

- jxl-oxide（rayon 多线程）：251 ms（24 MP/s）
- libjxl（clang-cl，8 线程）：75 ms（80 MP/s）——快约 3.4 倍

libjxl 为 C++ 参考实现（BSD-3/Apache），手写 SSE4/AVX2 SIMD + 多线程
并行 runner；jxl-oxide 无 SIMD 内联。常用绑定 jpegxl-rs / jpegxl-sys 为
GPL-3，本项目含商业许可体系，不可引入。

## 决策

1. **主引擎 = vendored libjxl 0.12**，经手写 FFI（`decode/jxl/libjxl_ffi.rs`，
   仅声明解码所需最小面 + 测试用编码器面）。源码来自 `jpegxl-src =0.12.0`
   （BSD-3，crate 元数据为准；其 lib.rs 文件头 GPL 字样系上游改协议的历史遗留），
   版本锁定保证可复现。构建逻辑在本项目 build.rs，不用 jpegxl-src::build()
   （其 Windows 路径硬编码 ClangCL VS 组件，未安装即失败）。
2. **Windows 编译器选型：clang-cl（自动比选最新版）+ Ninja 生成器**。
   MSVC cl 构建的 libjxl 因缺少 per-file `/arch` 支持导致 SIMD 派发失效，
   实测慢约 90 倍（48.7ms → 4.4s @6MP），不可用。无 clang-cl 时回退
   MSVC + 全局 `/arch:AVX2`。CRT 用 libjxl 默认动态 /MD，与 Rust MSVC
   工具链一致（此前 /MT + /Zl 方案引发 try-compile 链接失败与 LNK4098，已弃）。
3. **jxl-oxide 保留为回退引擎**（`--no-default-features` 移除 libjxl；
   或运行时单文件解码失败自动回退），覆盖 libjxl 兼容性边角。
4. **动画帧走原始 RGBA 直通 IPC**（`LoadResult::animated_rgba`，mime
   `image/x-rgba`），前端 ImageData→ImageBitmap 零转码；总原始体积超过
   128 MiB 才回退 PNG 快速压缩（防传输峰值爆内存）。静态图 RGBA 直通不变。
5. libjxl 侧输出 RGBA8，sRGB 输出档于 `JXL_DEC_COLOR_ENCODING` 事件时点经
   `JxlDecoderSetPreferredColorProfile` 请求：无 CMS 时 libjxl 仅对 XYB 码流
   执行 sRGB 转换；非 XYB 且非 sRGB 的文件（宽色域/嵌入式 ICC）该调用报错，
   此时保留原生色彩空间输出继续解码（宽色域文件仅轻微偏色）。接入内置 CMS
   （jxl_cms）可实现全量 sRGB 转换，但 FFI 面显著扩大，暂不做。

## 后果

- 构建机新增依赖：cmake + C++ 工具链（Windows 建议 clang-cl ≥ 19 + Ninja）。
  关闭特性可退化为纯 Rust。
- 二进制体积增加（libjxl + highway + brotli 静态链接，约数 MB）。
- FFI 面为手写转录，libjxl 升级时需对照官方头文件核对（结构体逐字段）。
- 性能回归基准：`cargo test jxl_engine_bench -- --nocapture`。
