//! JPEG XL 解码通道
//!
//! 引擎编排：libjxl（SIMD + 多线程并行 runner，`jxl-libjxl` 特性默认启用）
//! 失败自动回退 jxl-oxide（纯 Rust）；两引擎统一产出 RGBA8 帧序列。
//!
//! 输出通道：静态 RGBA8 直通 / 动画帧序列——小图走原始 RGBA 帧直通
//! （免二次编解码），超出内存预算或超尺寸时回退 PNG 快速压缩。
//! （Chromium / WebView2 不支持 JXL，走 Rust IPC 通道原始像素直通）

use std::io::Cursor;

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder};

use super::{cap_dimensions, LoadResult, MAX_DIM};

/// JXL 动画帧未指定延迟或异常时的默认帧间隔（100ms = 10fps）
pub(super) const DEFAULT_ANIM_FRAME_DELAY_MS: u32 = 100;

/// 帧间隔 ticks → ms（两引擎同公式；无时钟信息回退 DEFAULT_ANIM_FRAME_DELAY_MS）
fn ticks_to_ms(dur: u32, tps_numer: u32, tps_denom: u32) -> u32 {
    if tps_numer == 0 || dur == 0 {
        DEFAULT_ANIM_FRAME_DELAY_MS
    } else {
        ((dur as u64 * tps_denom as u64 * 1000) / tps_numer as u64).max(1) as u32
    }
}

/// 动画原始 RGBA 帧总预算：超出则回退 PNG 快速压缩，防多帧大图的信封内存爆炸
/// （128 MiB ≈ 720p×120 帧；动画收尾仍是等量 ImageBitmap，仅压低传输峰值）
const RAW_ANIM_BUDGET_BYTES: usize = 128 * 1024 * 1024;

/// 引擎无关的解码产物：RGBA8 帧序列（已按 MAX_DIM 封顶，静态恰好 1 帧）
pub(in crate::decode) struct EngineOutput {
    pub animated: bool,
    pub width: u32,
    pub height: u32,
    /// 每帧 width*height*4 字节 RGBA8
    pub frames: Vec<Vec<u8>>,
    /// animated 时与 frames 等长（ms）；静态为 [0]
    pub delays_ms: Vec<u32>,
}

#[cfg(feature = "jxl-libjxl")]
pub(super) mod libjxl_engine;
#[cfg(feature = "jxl-libjxl")]
pub(super) mod libjxl_ffi;

/// 轻量探测 JXL 是否为动画（仅解析头部元数据，不解码帧）
pub(super) fn is_jxl_animated(path: &str) -> bool {
    #[cfg(feature = "jxl-libjxl")]
    if let Some(v) = libjxl_engine::probe_animated(path) {
        return v;
    }
    oxide_probe_animated(path)
}

/// JXL 通道入口：解码 + 收尾打包；`Ok.1` 为引擎回退说明（None = 未回退），
/// 交分发点记录（解码通道内部零 log，见 docs/logging.md）
pub(super) fn decode_jxl(path: &str) -> Result<(LoadResult, Option<String>), String> {
    let (out, fallback) = decode_with_engine(path)?;
    Ok((finish(out), fallback))
}

/// 引擎选择：libjxl 优先，个别文件兼容性失败自动回退 jxl-oxide。
/// 回退成功时把 libjxl 失败原因交给分发点记 warn；双引擎皆失败时把
/// libjxl 原因并入错误串上浮，避免只留 oxide 报错丢线索
#[cfg(feature = "jxl-libjxl")]
fn decode_with_engine(path: &str) -> Result<(EngineOutput, Option<String>), String> {
    match libjxl_engine::decode(path) {
        Ok(out) => Ok((cap_engine_output(out), None)),
        Err(libjxl_err) => {
            let out = cap_engine_output(oxide_decode(path).map_err(|oxide_err| {
                format!("{oxide_err}（libjxl 主引擎亦失败: {libjxl_err}）")
            })?);
            Ok((out, Some(libjxl_err)))
        }
    }
}

#[cfg(not(feature = "jxl-libjxl"))]
fn decode_with_engine(path: &str) -> Result<(EngineOutput, Option<String>), String> {
    Ok((oxide_decode(path)?, None))
}

/// libjxl 引擎产物统一按 MAX_DIM 封顶（jxl-oxide 通道已在 oxide_decode 内逐帧封顶）
#[cfg(feature = "jxl-libjxl")]
fn cap_engine_output(mut out: EngineOutput) -> EngineOutput {
    if out.width.max(out.height) <= MAX_DIM {
        return out;
    }
    let (mut w, mut h) = (out.width, out.height);
    let mut frames = Vec::with_capacity(out.frames.len());
    for rgba in out.frames.drain(..) {
        let (capped, cw, ch) = cap_rgba_frame(w, h, rgba);
        if frames.is_empty() {
            w = cw;
            h = ch;
        }
        frames.push(capped);
    }
    out.width = w;
    out.height = h;
    out.frames = frames;
    out
}

/// 收尾打包：静态直通 / 动画按预算选 RGBA 直通或 PNG 快速压缩
fn finish(out: EngineOutput) -> LoadResult {
    if out.animated {
        finish_animation(out)
    } else {
        let mut frames = out.frames;
        let rgba = if frames.is_empty() {
            Vec::new()
        } else {
            frames.remove(0)
        };
        LoadResult::rgba("static", rgba, out.width, out.height, false)
    }
}

fn finish_animation(out: EngineOutput) -> LoadResult {
    let (w, h) = (out.width, out.height);
    let frame_bytes = (w as usize) * (h as usize) * 4;
    let total = frame_bytes.saturating_mul(out.frames.len());

    if total <= RAW_ANIM_BUDGET_BYTES {
        return LoadResult::animated_rgba(out.frames, out.delays_ms, w, h);
    }

    // 超预算：PNG 快速压缩回退（体积优先于编码速度，此时内存压力是主要矛盾）
    let mut pngs = Vec::with_capacity(out.frames.len());
    for rgba in &out.frames {
        let mut png = Vec::new();
        let enc = PngEncoder::new_with_quality(
            Cursor::new(&mut png),
            CompressionType::Fast,
            FilterType::Adaptive,
        );
        enc.write_image(rgba, w, h, ExtendedColorType::Rgba8)
            .map_err(|e| format!("JXL 帧 PNG 编码失败: {e}"))
            .expect("内存内 PNG 编码不应失败");
        pngs.push(png);
    }
    LoadResult::animated(pngs, out.delays_ms)
}

// ============================================================
// jxl-oxide 回退引擎
// ============================================================

#[inline]
fn has_animation(image: &jxl_oxide::JxlImage) -> bool {
    image.image_header().metadata.animation.is_some()
}

/// jxl-oxide 轻量探测（仅解析容器与头部元数据，不解码帧）
fn oxide_probe_animated(path: &str) -> bool {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut reader = std::io::BufReader::new(file);
    let mut uninit = jxl_oxide::JxlImage::builder().build_uninit();
    let mut buf = [0u8; 4096];
    let mut buf_valid = 0usize;

    // 限制最多读取 64KB，保证轻量探测不产生大 IO 开销
    let mut total_read = 0usize;
    const MAX_PROBE_BYTES: usize = 64 * 1024;

    while total_read < MAX_PROBE_BYTES {
        use std::io::Read;
        let count = match reader.read(&mut buf[buf_valid..]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => return false,
        };
        buf_valid += count;
        total_read += count;

        let consumed = match uninit.feed_bytes(&buf[..buf_valid]) {
            Ok(n) => n,
            Err(_) => return false,
        };
        buf.copy_within(consumed..buf_valid, 0);
        buf_valid -= consumed;

        match uninit.try_init() {
            Ok(jxl_oxide::InitializeResult::Initialized(image)) => {
                return has_animation(&image);
            }
            Ok(jxl_oxide::InitializeResult::NeedMoreData(next_uninit)) => {
                uninit = next_uninit;
            }
            Err(_) => return false,
        }
    }
    false
}

#[inline]
fn cmyk_to_rgb(c: u8, m: u8, y: u8, k: u8) -> (u8, u8, u8) {
    let c = c as f32 / 255.0;
    let m = m as f32 / 255.0;
    let y = y as f32 / 255.0;
    let k = k as f32 / 255.0;
    let r = ((1.0 - c) * (1.0 - k) * 255.0).round() as u8;
    let g = ((1.0 - m) * (1.0 - k) * 255.0).round() as u8;
    let b = ((1.0 - y) * (1.0 - k) * 255.0).round() as u8;
    (r, g, b)
}

fn render_to_rgba(
    image: &jxl_oxide::JxlImage,
    render: &jxl_oxide::Render,
) -> Result<Vec<u8>, String> {
    let mut stream = render.stream();
    let (w, h, ch) = (stream.width(), stream.height(), stream.channels());
    let mut buf = vec![0u8; (w * h * ch) as usize];
    stream.write_to_buffer(&mut buf);

    let rgba: Vec<u8> = match image.pixel_format() {
        jxl_oxide::PixelFormat::Gray => buf,
        jxl_oxide::PixelFormat::Graya => buf
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        jxl_oxide::PixelFormat::Rgb => buf
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        jxl_oxide::PixelFormat::Rgba => buf,
        jxl_oxide::PixelFormat::Cmyk => buf
            .chunks_exact(4)
            .flat_map(|p| {
                let (r, g, b) = cmyk_to_rgb(p[0], p[1], p[2], p[3]);
                [r, g, b, 255]
            })
            .collect(),
        jxl_oxide::PixelFormat::Cmyka => buf
            .chunks_exact(5)
            .flat_map(|p| {
                let (r, g, b) = cmyk_to_rgb(p[0], p[1], p[2], p[3]);
                [r, g, b, p[4]]
            })
            .collect(),
    };
    Ok(rgba)
}

fn oxide_decode(path: &str) -> Result<EngineOutput, String> {
    let image = jxl_oxide::JxlImage::builder()
        .open(path)
        .map_err(|e| format!("JXL 打开失败: {e}"))?;

    let is_anim = has_animation(&image);
    let num_keyframes = image.num_loaded_keyframes();

    if is_anim && num_keyframes >= 2 {
        let anim = image
            .image_header()
            .metadata
            .animation
            .as_ref()
            .ok_or_else(|| "非动画 JXL".to_string())?;
        let (tps_numer, tps_denom) = (anim.tps_numerator, anim.tps_denominator);

        let mut frames = Vec::with_capacity(num_keyframes);
        let mut delays_ms = Vec::with_capacity(num_keyframes);
        let mut dims = (0u32, 0u32);

        for idx in 0..num_keyframes {
            let render = image
                .render_frame(idx)
                .map_err(|e| format!("JXL 帧 {idx} 渲染失败: {e}"))?;
            let rgba = render_to_rgba(&image, &render)?;
            let (w, h) = (render.stream().width(), render.stream().height());
            let (rgba, w, h) = cap_rgba_frame(w, h, rgba);
            if dims == (0, 0) {
                dims = (w, h);
            }
            let dur = render.duration();
            let delay_ms = ticks_to_ms(dur, tps_numer, tps_denom);
            frames.push(rgba);
            delays_ms.push(delay_ms);
        }

        Ok(EngineOutput {
            animated: true,
            width: dims.0,
            height: dims.1,
            frames,
            delays_ms,
        })
    } else {
        let render = image
            .render_frame(0)
            .map_err(|e| format!("JXL 渲染失败: {e}"))?;
        let (w, h) = (render.stream().width(), render.stream().height());
        let rgba = render_to_rgba(&image, &render)?;
        let (rgba, w, h) = cap_rgba_frame(w, h, rgba);
        Ok(EngineOutput {
            animated: false,
            width: w,
            height: h,
            frames: vec![rgba],
            delays_ms: vec![0],
        })
    }
}

/// RGBA 帧按 MAX_DIM 封顶：超出时长边缩至上限并重采像素，返回（像素, 新宽, 新高）
fn cap_rgba_frame(w: u32, h: u32, rgba: Vec<u8>) -> (Vec<u8>, u32, u32) {
    if w.max(h) <= MAX_DIM || rgba.is_empty() {
        return (rgba, w, h);
    }
    let img = image::RgbaImage::from_raw(w, h, rgba)
        .map(image::DynamicImage::ImageRgba8)
        .expect("RGBA 帧长度与尺寸匹配");
    let capped = cap_dimensions(img);
    let (cw, ch) = (capped.width(), capped.height());
    let rgba = capped.into_rgba8().into_raw();
    (rgba, cw, ch)
}
