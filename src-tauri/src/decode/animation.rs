//! 动画拆帧通道：GIF / APNG / 动态 WebP → 帧 PNG 字节 + 每帧延迟
//! 帧数 < 2 视为静态返回 None（原生 <img> 播放）。
//! 默认动画由浏览器原生 <img> 播放（零 Rust 开销、首帧即时）；本通道仅在
//! 用户需要逐帧控制（暂停/步进）时被调用，结果缓存于 DecodeCache。

use std::fs::File;
use std::io::{BufReader, Cursor};

use image::AnimationDecoder;

use super::LoadResult;

pub(super) fn decode_animation(path: &str) -> Result<Option<LoadResult>, String> {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let reader = BufReader::new(File::open(path).map_err(|e| e.to_string())?);
    // 拆帧重编码会丢 EXIF 方向标记（原生播放由浏览器自动应用方向），
    // 必须在编码前按 EXIF 回正（WebP EXIF / PNG eXIf；GIF 无 EXIF → 1）
    let orientation = super::preview::read_orientation(path).unwrap_or(1);

    match ext.as_str() {
        "gif" => {
            let decoder = image::codecs::gif::GifDecoder::new(reader).map_err(|e| e.to_string())?;
            collect_frames(decoder.into_frames(), orientation)
        }
        "png" => {
            let decoder = image::codecs::png::PngDecoder::new(reader).map_err(|e| e.to_string())?;
            if !decoder.is_apng().map_err(|e| e.to_string())? {
                // 静态 PNG：原生 <img>，无帧可拆
                return Ok(None);
            }
            let apng = decoder.apng().map_err(|e| e.to_string())?;
            collect_frames(apng.into_frames(), orientation)
        }
        "webp" => {
            let decoder = image::codecs::webp::WebPDecoder::new(reader).map_err(|e| e.to_string())?;
            collect_frames(decoder.into_frames(), orientation)
        }
        _ => Ok(None),
    }
}

/// 前两帧仅解码不编码：帧数 < 2 直接判定静态，避免无谓 PNG 编码；
/// 确认多帧后边解码边编码、逐帧释放（长 GIF 峰值内存 ≈ ~2 帧）
fn collect_frames(
    iter: image::Frames<'_>,
    orientation: u8,
) -> Result<Option<LoadResult>, String> {
    let mut frame_iter = iter;
    let Some(first) = frame_iter.next() else {
        return Ok(None);
    };
    let first = first.map_err(|e| format!("动画帧解码失败: {e}"))?;
    let Some(second) = frame_iter.next() else {
        return Ok(None);
    };
    let second = second.map_err(|e| format!("动画帧解码失败: {e}"))?;

    let mut frames = Vec::new();
    let mut delays = Vec::new();
    let (buf0, d0) = encode_frame(first, orientation)?;
    let (buf1, d1) = encode_frame(second, orientation)?;
    frames.push(buf0);
    frames.push(buf1);
    delays.push(d0);
    delays.push(d1);
    for frame in frame_iter {
        let frame = frame.map_err(|e| format!("动画帧解码失败: {e}"))?;
        let (buf, d) = encode_frame(frame, orientation)?;
        frames.push(buf);
        delays.push(d);
    }
    Ok(Some(LoadResult::animated(frames, delays)))
}

/// 单帧 → PNG 字节 + 延迟（GIF 规范：0 延迟按 100ms；den 为 0 时兜底）
pub(super) fn encode_frame(frame: image::Frame, orientation: u8) -> Result<(Vec<u8>, u32), String> {
    let (num, den) = frame.delay().numer_denom_ms();
    // 整数除法可截断为 0（如 5/10ms）：max(1) 兜底，避免前端 setTimeout(0) 全速疯转
    let delay_ms = if den == 0 || num == 0 {
        100
    } else {
        (num / den).max(1)
    };
    let img = super::preview::apply_orientation(
        image::DynamicImage::ImageRgba8(frame.into_buffer()),
        orientation,
    );
    let mut png: Vec<u8> = Vec::new();
    img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| format!("帧 PNG 编码失败: {e}"))?;
    Ok((png, delay_ms))
}
