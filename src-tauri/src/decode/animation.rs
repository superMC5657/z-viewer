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
    let mut frame_iter = iter;

    // 前两帧仅解码不编码：帧数 < 2 直接判定静态，避免无谓 PNG 编码
    let Some(first) = frame_iter.next() else {
        return Ok(None);
    };
    let first = first.map_err(|e| format!("动画帧解码失败: {e}"))?;
    let Some(second) = frame_iter.next() else {
        return Ok(None);
    };
    let second = second.map_err(|e| format!("动画帧解码失败: {e}"))?;

    // 确认多帧后边解码边编码、逐帧释放：长 GIF 峰值内存 = ~2 帧（而非全部帧驻留）
    let mut frames = Vec::new();
    frames.push(encode_frame(first)?);
    frames.push(encode_frame(second)?);
    for frame in frame_iter {
        let frame = frame.map_err(|e| format!("动画帧解码失败: {e}"))?;
        frames.push(encode_frame(frame)?);
    }
    Ok(Some(frames))
}

/// 单帧 → base64 PNG + 延迟（GIF 规范：0 延迟按 100ms；den 为 0 时兜底）
pub(super) fn encode_frame(frame: image::Frame) -> Result<FrameData, String> {
    let (num, den) = frame.delay().numer_denom_ms();
    // 整数除法可截断为 0（如 5/10ms）：max(1) 兜底，避免前端 setTimeout(0) 全速疯转
    let delay_ms = if den == 0 || num == 0 {
        100
    } else {
        (num / den).max(1)
    };
    let mut png: Vec<u8> = Vec::new();
    image::DynamicImage::ImageRgba8(frame.into_buffer())
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| format!("帧 PNG 编码失败: {e}"))?;
    Ok(FrameData {
        png: b64(&png),
        delay_ms,
    })
}
