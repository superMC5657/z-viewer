//! 动画拆帧通道：GIF / APNG / 动态 WebP → 帧 PNG + 每帧延迟
//! 帧数 < 2 视为静态返回 None（降级 asset 通道）

use std::fs::File;
use std::io::{BufReader, Cursor};

use image::AnimationDecoder;

use super::{b64, FrameData};

pub(super) fn decode_animation(path: &str) -> Result<Option<Vec<FrameData>>, String> {
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
