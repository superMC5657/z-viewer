//! 解码服务：RAW 解码（rawler）+ 动画拆帧（image crate）
//!
//! 图片加载通道（《需求报告与技术方案.md》8.3）：
//! - 常见格式 → 前端 asset 协议直读（零拷贝）
//! - RAW → Rust 解码 → 降采样 → JPEG 字节 → 前端 Blob URL
//! - 动画（GIF/APNG/动态 WebP）→ Rust 拆帧 → 帧 PNG + 每帧延迟 → 前端 canvas 逐帧控制

use std::fs::File;
use std::io::{BufReader, Cursor};

use base64::Engine;
use image::AnimationDecoder;
use serde::Serialize;

/// 相机 RAW 扩展名（rawler 覆盖主流机型；供 browse.rs 浏览枚举复用）
pub const RAW_EXTS: &[&str] = &[
    "cr2", "cr3", "nef", "arw", "dng", "orf", "rw2", "pef", "srw", "raf", "raw", "x3f", "erf",
    "3fr", "kdc", "dcr", "mrw", "mef", "mos", "iiq", "fff", "ari",
];

/// 潜在动画格式（按帧数判定是否真的多帧）
const ANIM_EXTS: &[&str] = &["gif", "png", "webp"];

/// RAW 解码后降采样最长边限制（屏幕 2 倍尺寸足够，控制传输体积）
const MAX_DIM: u32 = 2560;

#[derive(Serialize, Clone)]
pub struct FrameData {
    /// base64 编码的 PNG 帧
    pub png: String,
    pub delay_ms: u32,
}

#[derive(Serialize, Clone)]
pub struct LoadResult {
    /// "asset"（前端 asset 协议直读）| "raw"（解码 JPEG）| "animated"（帧序列）
    pub mode: String,
    /// raw 模式：base64 编码的 JPEG 字节
    pub data: Option<String>,
    /// animated 模式：帧序列
    pub frames: Option<Vec<FrameData>>,
}

/// 统一入口：按扩展名与内容分发到三条加载通道
pub fn load_image(path: &str) -> Result<LoadResult, String> {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();

    if is_raw_ext(&ext) {
        return decode_raw(path);
    }

    if ANIM_EXTS.contains(&ext.as_str()) {
        if let Some(frames) = decode_animation(path)? {
            return Ok(LoadResult {
                mode: "animated".into(),
                data: None,
                frames: Some(frames),
            });
        }
        // 单帧：降级为静态，走 asset 协议直读
    }

    Ok(LoadResult {
        mode: "asset".into(),
        data: None,
        frames: None,
    })
}

pub fn is_raw_ext(ext: &str) -> bool {
    RAW_EXTS.contains(&ext.to_ascii_lowercase().as_str())
}

/// 浏览器原生解码的 asset 扩展名（前端 PrefetchPool 预热，Rust 预取跳过）
pub fn is_asset_ext(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "jpg" | "jpeg" | "bmp" | "ico" | "svg")
}

/// RAW 解码 → develop（demosaic + 白平衡 + 色彩校准 + sRGB）→ 降采样 → JPEG
fn decode_raw(path: &str) -> Result<LoadResult, String> {
    let rawimage = rawler::decode_file(path).map_err(|e| format!("RAW 解码失败: {e}"))?;
    let develop = rawler::imgop::develop::RawDevelop::default();
    let intermediate = develop
        .develop_intermediate(&rawimage)
        .map_err(|e| format!("RAW 处理失败: {e}"))?;
    let img = intermediate
        .to_dynamic_image()
        .ok_or_else(|| "RAW 图像转换失败".to_string())?;

    let (w, h) = (img.width(), img.height());
    let img = if w.max(h) > MAX_DIM {
        let scale = MAX_DIM as f32 / w.max(h) as f32;
        img.resize(
            (w as f32 * scale).round() as u32,
            (h as f32 * scale).round() as u32,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let img = img.to_rgb8();

    let mut buf: Vec<u8> = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Jpeg)
        .map_err(|e| format!("JPEG 编码失败: {e}"))?;

    Ok(LoadResult {
        mode: "raw".into(),
        data: Some(b64(&buf)),
        frames: None,
    })
}

/// 动画拆帧：GIF / APNG / 动态 WebP；帧数 < 2 视为静态返回 None
fn decode_animation(path: &str) -> Result<Option<Vec<FrameData>>, String> {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let reader = BufReader::new(File::open(path).map_err(|e| e.to_string())?);

    match ext.as_str() {
        "gif" => {
            let decoder = image::codecs::gif::GifDecoder::new(reader).map_err(|e| e.to_string())?;
            collect_frames(decoder.into_frames())
        }
        "png" => {
            let decoder = image::codecs::png::PngDecoder::new(reader).map_err(|e| e.to_string())?;
            if !decoder.is_apng().map_err(|e| e.to_string())? {
                // 静态 PNG：走 asset 协议直读
                return Ok(None);
            }
            let apng = decoder.apng().map_err(|e| e.to_string())?;
            collect_frames(apng.into_frames())
        }
        "webp" => {
            let decoder =
                image::codecs::webp::WebPDecoder::new(reader).map_err(|e| e.to_string())?;
            collect_frames(decoder.into_frames())
        }
        _ => Ok(None),
    }
}

fn collect_frames(iter: image::Frames<'_>) -> Result<Option<Vec<FrameData>>, String> {
    // 先解码帧（不编码 PNG），帧数 < 2 时直接判定静态，避免无谓编码
    let mut decoded: Vec<(image::RgbaImage, u32)> = Vec::new();
    for frame in iter {
        let frame = frame.map_err(|e| format!("动画帧解码失败: {e}"))?;
        let (num, den) = frame.delay().numer_denom_ms();
        // GIF 规范：0 延迟按 100ms；den 为 0 时兜底
        let delay_ms = if den == 0 || num == 0 { 100 } else { num / den };
        decoded.push((frame.into_buffer(), delay_ms));
    }
    if decoded.len() < 2 {
        return Ok(None);
    }
    let mut frames = Vec::with_capacity(decoded.len());
    for (buf, delay_ms) in decoded {
        let mut png: Vec<u8> = Vec::new();
        image::DynamicImage::ImageRgba8(buf)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| format!("帧 PNG 编码失败: {e}"))?;
        frames.push(FrameData {
            png: b64(&png),
            delay_ms,
        });
    }
    Ok(Some(frames))
}

fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试图库相对路径（从 src-tauri 工作目录运行）
    fn test_img(rel: &str) -> String {
        std::env::current_dir()
            .unwrap()
            .join("..")
            .join(rel)
            .to_string_lossy()
            .to_string()
    }

    #[test]
    fn gif_animation_frames() {
        let result = load_image(&test_img("test-images/B/img_2.gif")).unwrap();
        assert_eq!(result.mode, "animated", "两帧 GIF 应识别为动画");
        let frames = result.frames.expect("动画帧应有数据");
        assert!(frames.len() >= 2, "GIF 至少 2 帧，实际 {}", frames.len());
        for f in &frames {
            assert!(!f.png.is_empty(), "帧 PNG 不应为空");
            assert!(f.delay_ms > 0, "帧延迟应 > 0，实际 {}", f.delay_ms);
        }
        // 测试生成的 GIF 每帧 400ms
        assert_eq!(frames[0].delay_ms, 400);
    }

    #[test]
    fn static_png_goes_asset() {
        let result = load_image(&test_img("test-images/A/1.png")).unwrap();
        assert_eq!(result.mode, "asset");
        assert!(result.data.is_none());
        assert!(result.frames.is_none());
    }

    #[test]
    fn raw_ext_detection() {
        assert!(is_raw_ext("CR2"));
        assert!(is_raw_ext("nef"));
        assert!(is_raw_ext("DNG"));
        assert!(!is_raw_ext("jpg"));
        assert!(!is_raw_ext("png"));
    }
}
