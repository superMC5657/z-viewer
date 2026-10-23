//! JPEG XL 解码通道：jxl-oxide 解码 → 静态 RGBA8 直通 / 动画帧序列
//! （Chromium / WebView2 不支持 JXL，走 Rust IPC 通道原始像素直通，免二次编解码开销）

use std::io::Cursor;

use super::{cap_dimensions, LoadResult};

/// JXL 动画帧未指定延迟或异常时的默认帧间隔（100ms = 10fps）
const DEFAULT_ANIM_FRAME_DELAY_MS: u32 = 100;

#[inline]
fn has_animation(image: &jxl_oxide::JxlImage) -> bool {
    image.image_header().metadata.animation.is_some()
}

/// 轻量探测 JXL 是否为动画（仅解析容器与头部元数据，不解码帧）
pub(super) fn is_jxl_animated(path: &str) -> bool {
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

pub(super) fn decode_jxl(path: &str) -> Result<LoadResult, String> {
    let image = jxl_oxide::JxlImage::builder()
        .open(path)
        .map_err(|e| format!("JXL 打开失败: {e}"))?;

    let is_anim = has_animation(&image);
    let num_keyframes = image.num_loaded_keyframes();

    if is_anim && num_keyframes >= 2 {
        decode_animation(&image)
    } else {
        decode_static(&image)
    }
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

fn render_to_dynamic_image(
    image: &jxl_oxide::JxlImage,
    render: &jxl_oxide::Render,
) -> Result<image::DynamicImage, String> {
    let mut stream = render.stream();
    let (w, h, ch) = (stream.width(), stream.height(), stream.channels());
    let mut buf = vec![0u8; (w * h * ch) as usize];
    stream.write_to_buffer(&mut buf);

    match image.pixel_format() {
        jxl_oxide::PixelFormat::Gray => {
            image::GrayImage::from_raw(w, h, buf).map(image::DynamicImage::ImageLuma8)
        }
        jxl_oxide::PixelFormat::Graya => {
            image::GrayAlphaImage::from_raw(w, h, buf).map(image::DynamicImage::ImageLumaA8)
        }
        jxl_oxide::PixelFormat::Rgb => {
            image::RgbImage::from_raw(w, h, buf).map(image::DynamicImage::ImageRgb8)
        }
        jxl_oxide::PixelFormat::Rgba => {
            image::RgbaImage::from_raw(w, h, buf).map(image::DynamicImage::ImageRgba8)
        }
        jxl_oxide::PixelFormat::Cmyk => {
            let mut rgb = Vec::with_capacity((w * h * 3) as usize);
            for chunk in buf.chunks_exact(4) {
                let (r, g, b) = cmyk_to_rgb(chunk[0], chunk[1], chunk[2], chunk[3]);
                rgb.push(r);
                rgb.push(g);
                rgb.push(b);
            }
            image::RgbImage::from_raw(w, h, rgb).map(image::DynamicImage::ImageRgb8)
        }
        jxl_oxide::PixelFormat::Cmyka => {
            let mut rgba = Vec::with_capacity((w * h * 4) as usize);
            for chunk in buf.chunks_exact(5) {
                let (r, g, b) = cmyk_to_rgb(chunk[0], chunk[1], chunk[2], chunk[3]);
                let a = chunk[4];
                rgba.push(r);
                rgba.push(g);
                rgba.push(b);
                rgba.push(a);
            }
            image::RgbaImage::from_raw(w, h, rgba).map(image::DynamicImage::ImageRgba8)
        }
    }
    .ok_or_else(|| "JXL 缓冲区转换失败".to_string())
}

fn decode_static(image: &jxl_oxide::JxlImage) -> Result<LoadResult, String> {
    let render = image
        .render_frame(0)
        .map_err(|e| format!("JXL 渲染失败: {e}"))?;
    let dyn_img = render_to_dynamic_image(image, &render)?;
    let img = cap_dimensions(dyn_img);
    let (w, h) = (img.width(), img.height());
    let rgba = img.to_rgba8().into_raw();
    Ok(LoadResult::rgba("static", rgba, w, h, false))
}

fn decode_animation(image: &jxl_oxide::JxlImage) -> Result<LoadResult, String> {
    let anim = image
        .image_header()
        .metadata
        .animation
        .as_ref()
        .ok_or_else(|| "非动画 JXL".to_string())?;

    let tps_numer = anim.tps_numerator;
    let tps_denom = anim.tps_denominator;
    let num_keyframes = image.num_loaded_keyframes();

    let mut frames = Vec::with_capacity(num_keyframes);
    let mut delays = Vec::with_capacity(num_keyframes);

    for idx in 0..num_keyframes {
        let render = image
            .render_frame(idx)
            .map_err(|e| format!("JXL 帧 {idx} 渲染失败: {e}"))?;
        let dyn_img = render_to_dynamic_image(image, &render)?;
        let img = cap_dimensions(dyn_img);

        let dur = render.duration();
        let delay_ms = if tps_numer == 0 || dur == 0 {
            DEFAULT_ANIM_FRAME_DELAY_MS
        } else {
            ((dur as u64 * tps_denom as u64 * 1000) / tps_numer as u64).max(1) as u32
        };

        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| format!("JXL 帧 {idx} PNG 编码失败: {e}"))?;

        frames.push(png);
        delays.push(delay_ms);
    }

    Ok(LoadResult::animated(frames, delays))
}
