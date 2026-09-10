//! TIFF 静态解码通道：image crate 解码 → 方向回正 → 降采样 → 原始 RGBA8 像素
//! （Chromium 不解码 TIFF，走 Rust 通道原始像素直通，免二次 JPEG 有损压缩与编解码开销）

use super::{cap_dimensions, LoadResult};

pub(super) fn decode_static(path: &str) -> Result<LoadResult, String> {
    use std::io::{BufReader, Seek, SeekFrom};

    let mut file = std::fs::File::open(path).map_err(|e| format!("TIFF 打开失败: {e}"))?;
    // 复用同一文件句柄：EXIF 头在文件前部，读方向后 seek 回开头再解码（省一次 open）
    let orientation = super::preview::read_orientation(&file).unwrap_or(1);
    file.seek(SeekFrom::Start(0))
        .map_err(|e| format!("TIFF 打开失败: {e}"))?;
    let img = image::ImageReader::new(BufReader::new(&file))
        .with_guessed_format()
        .map_err(|e| format!("TIFF 打开失败: {e}"))?
        .decode()
        .map_err(|e| format!("TIFF 解码失败: {e}"))?;
    let img = cap_dimensions(super::preview::apply_orientation(img, orientation));
    let (w, h) = (img.width(), img.height());
    let rgba = img.to_rgba8();

    Ok(LoadResult::rgba("static", rgba.into_raw(), w, h, false))
}
