//! TIFF 静态解码通道：image crate 解码 → 方向回正 → 降采样 → 原始 RGBA8 像素
//! （Chromium 不解码 TIFF，走 Rust 通道原始像素直通，免二次 JPEG 有损压缩与编解码开销）

use super::{finalize_rgba, load_mode, LoadResult};

pub(super) fn decode_static(path: &str, cap: u32) -> Result<LoadResult, String> {
    use std::io::BufReader;

    // EXIF 头在文件前部：open_with_orientation 读方向后已 seek 回开头（省一次 open）
    let (file, orientation) =
        super::preview::open_with_orientation(path).map_err(|e| format!("TIFF 打开失败: {e}"))?;
    let img = image::ImageReader::new(BufReader::new(&file))
        .with_guessed_format()
        .map_err(|e| format!("TIFF 打开失败: {e}"))?
        .decode()
        .map_err(|e| format!("TIFF 解码失败: {e}"))?;

    Ok(finalize_rgba(img, orientation, load_mode::STATIC, cap))
}
